// The counter example, driven through the unmodified Phoenix client in a real browser.

import { createHmac } from "node:crypto";

import { expect, test } from "@playwright/test";

// Every test also requires a quiet console: an error or a warning there, such as the
// client's complaint about a server of another version, fails the test.
let complaints;
test.beforeEach(({ page }) => {
  complaints = [];
  page.on("console", (message) => {
    if (["error", "warning"].includes(message.type())) {
      complaints.push(`${message.type()}: ${message.text()}`);
    }
  });
  page.on("pageerror", (error) => complaints.push(`uncaught: ${error.message}`));
});
test.afterEach(() => expect(complaints).toEqual([]));

test("clicking increment three times shows 3", async ({ page }) => {
  await page.goto("/");
  const count = page.locator("#count");
  await expect(count).toHaveText("0");
  await connected(page);

  const increment = page.getByRole("button", { name: "Increment" });
  await increment.click();
  await increment.click();
  await increment.click();

  await expect(count).toHaveText("3");
  // A page that never set a title must not read "undefined" once the client mounts.
  await expect(page).not.toHaveTitle("undefined");
});

test("each button's typed Event reaches the handler with its field", async ({ page }) => {
  await page.goto("/");
  const count = page.locator("#count");
  await connected(page);
  // What the binding of a typed Event wrote: the name, and the field beside it.
  const decrement = page.getByRole("button", { name: "Decrement" });
  await expect(decrement).toHaveAttribute("phx-click", "add");
  await expect(decrement).toHaveAttribute("phx-value-by", "-1");

  await page.getByRole("button", { name: "Increment" }).click();
  await expect(count).toHaveText("1");
  await decrement.click();
  await decrement.click();
  await expect(count).toHaveText("-1");
  await page.getByRole("button", { name: "Reset" }).click();
  await expect(count).toHaveText("0");
});

test("a LiveView ended by an Event it cannot decode is mounted afresh", async ({ page }) => {
  await page.goto("/");
  const count = page.locator("#count");
  const increment = page.getByRole("button", { name: "Increment" });
  const main = page.locator("[data-phx-main]");
  await connected(page);
  await increment.click();
  await expect(count).toHaveText("1");
  const container = await main.getAttribute("id");

  // A binding the counter's template never had, as a tampered page would send.
  const reset = page.getByRole("button", { name: "Reset" });
  await reset.evaluate((button) => button.setAttribute("phx-click", "no_such_event"));
  await reset.click();

  // The client is told its channel crashed, shows that, and joins again by itself.
  // The state went with the LiveView, so the new one starts from mount.
  await expect(main).toHaveClass(/phx-error/);
  await expect(count).toHaveText("0");
  await connected(page);
  await increment.click();
  await expect(count).toHaveText("1");
  await expect(main).toHaveAttribute("id", container);
});

test("the page works again after its socket is forced closed", async ({ page }) => {
  const sockets = [];
  page.on("websocket", (socket) => sockets.push(socket));
  await page.goto("/");
  const count = page.locator("#count");
  const increment = page.getByRole("button", { name: "Increment" });
  await connected(page);
  await increment.click();
  await expect(count).toHaveText("1");
  const container = await page.locator("[data-phx-main]").getAttribute("id");

  // Not the client's own `disconnect()`, which would not reconnect: the browser's
  // WebSocket is closed under the client, as a dropped connection looks to it.
  await page.evaluate(() => window.liveSocket.socket.conn.close());

  // The state lived in the task the closed socket ended, so the new Connected render
  // starts from mount again.
  await expect(count).toHaveText("0");
  await connected(page);
  await increment.click();
  await increment.click();
  await expect(count).toHaveText("2");
  // The same container: the client rejoined, it did not load the page again.
  await expect(page.locator("[data-phx-main]")).toHaveAttribute("id", container);
  expect(sockets.map((socket) => socket.isClosed())).toEqual([true, false]);
});

// The example's signing secret, which is public (`examples/counter/src/main.rs`), so
// a test can sign a render token as the server does and choose its age.
const EXAMPLE_SECRET = "griffin counter example: not a secret";
const TWO_WEEKS = 14 * 24 * 60 * 60 * 1000;

// A token in the format `griffin_web::token` documents, issued `age` milliseconds ago.
function signed(purpose, data, age) {
  const envelope = JSON.stringify({ purpose, issued_at: Date.now() - age, data });
  const payload = Buffer.from(envelope).toString("base64url");
  const signature = createHmac("sha256", EXAMPLE_SECRET).update(payload).digest("base64url");
  return `${payload}.${signature}`;
}

// Closes the browser's WebSocket under the client, as a dropped connection looks to
// it: the client connects again and joins with what the container carries then.
async function dropSocket(page) {
  await page.evaluate(() => window.liveSocket.socket.conn.close());
}

test("a page whose render token has expired is loaded again", async ({ page }) => {
  await page.goto("/");
  const count = page.locator("#count");
  const increment = page.getByRole("button", { name: "Increment" });
  const main = page.locator("[data-phx-main]");
  await connected(page);
  await increment.click();
  await expect(count).toHaveText("1");
  const container = await main.getAttribute("id");
  const sessionToken = (age) =>
    signed("live session", { id: container, view: "counter::Counter" }, age);

  // A token signed here is as good as the server's own while it is young: the client
  // joins again with it and the page is not loaded again.
  const young = sessionToken(TWO_WEEKS - 60_000);
  await main.evaluate((element, token) => element.setAttribute("data-phx-session", token), young);
  await dropSocket(page);
  await expect(count).toHaveText("0");
  await connected(page);
  await expect(main).toHaveAttribute("id", container);
  await expect(main).toHaveAttribute("data-phx-session", young);

  // Past its lifetime the join is refused, and the client falls back to a full page
  // load: a new Dead render, with a container and tokens of its own.
  const expired = sessionToken(TWO_WEEKS + 60_000);
  await main.evaluate((element, token) => element.setAttribute("data-phx-session", token), expired);
  await dropSocket(page);
  await expect(main).not.toHaveAttribute("id", container);
  await expect(main).not.toHaveAttribute("data-phx-session", expired);
  await connected(page);
  await increment.click();
  await expect(count).toHaveText("1");
});

test("the socket is opened with the CSRF token of the page's session", async ({ page }) => {
  const sockets = [];
  page.on("websocket", (socket) => sockets.push(socket.url()));
  await page.goto("/");
  const main = page.locator("[data-phx-main]");
  await connected(page);

  const token = await main.getAttribute("data-csrf-token");
  expect(token).toMatch(/^[\w-]{86}$/);
  expect(new URL(sockets[0]).searchParams.get("_csrf_token")).toBe(token);
});

test("a page whose session is gone is loaded again", async ({ page, context }) => {
  await page.goto("/");
  const count = page.locator("#count");
  const increment = page.getByRole("button", { name: "Increment" });
  const main = page.locator("[data-phx-main]");
  await connected(page);
  await increment.click();
  await expect(count).toHaveText("1");
  const container = await main.getAttribute("id");
  const token = await main.getAttribute("data-csrf-token");

  // The browser lost its session, as when it is signed out in another tab. The page
  // still carries the CSRF token of the old one, which the next socket is opened with.
  await context.clearCookies();
  await dropSocket(page);

  // No join is accepted on that socket, and the client loads the page again: a new
  // session, and a page that carries its token.
  await expect(main).not.toHaveAttribute("id", container);
  await expect(main).not.toHaveAttribute("data-csrf-token", token);
  await connected(page);
  await increment.click();
  await expect(count).toHaveText("1");
});

// The client marks the LiveView's container once the Connected render has arrived.
// Clicks before that reach nothing.
async function connected(page) {
  await expect(page.locator("[data-phx-main]")).toHaveClass(/phx-connected/);
}
