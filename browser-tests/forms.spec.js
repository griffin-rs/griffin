// The forms example: a live form and a controller form over one Changeset, through the
// unmodified Phoenix client in a real browser.

import { expect, test } from "@playwright/test";

// The example's server: four ports up from the counter's (`playwright.config.js`).
const port = Number(process.env.BROWSER_TEST_PORT ?? 4107) + 4;
test.use({ baseURL: `http://127.0.0.1:${port}` });

// As in `counter.spec.js`: an error or a warning in the console fails the test.
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

// A username no other test, and no earlier run against the same server, has used.
let counter = 0;
const unique = (prefix) => `${prefix}${Date.now()}${counter++}`.slice(0, 20);

// Text that would break out of an attribute and of an element if it were not escaped.
const hostile = `"><script>window.injected = 1</script><b id="injected">`;

test("errors show only for fields the user has used, and what was typed stays", async ({
  page,
}) => {
  await page.goto("/live");
  await connected(page);
  const email = page.locator("#signup_email");

  await email.fill("ada");

  // The email is used and does not parse. The username is empty, which is also an
  // error, but the user has not reached it: the client marked it unused.
  await expect(page.locator("#signup_email_error")).toContainText("must be an address");
  await expect(page.locator("#signup_username_error")).toHaveCount(0);
  await expect(email).toHaveValue("ada");
  await expect(email).toHaveAttribute("aria-invalid", "true");

  // Used now, the username shows its error.
  await page.locator("#signup_username").fill("x");
  await expect(page.locator("#signup_username_error")).toContainText("3 to 20");
  await expect(email).toHaveValue("ada");
});

test("an untouched checkbox group is sent as unused while the user types elsewhere", async ({
  page,
}) => {
  const frames = formFrames(page);
  await page.goto("/live");
  await connected(page);

  await page.locator("#signup_email").fill("ada");
  await expect.poll(() => frames.length).toBeGreaterThan(0);

  // The client marks the group unused (it has a visible input and no focus), so the
  // Changeset does not show its errors yet.
  const body = new URLSearchParams(frames[frames.length - 1]);
  expect(body.has("signup[_unused_topics][]")).toBe(true);
  expect(body.get("signup[_sent_topics]")).toBe("");
});

test("a value with quotes and markup is shown as text, in the input and out of it", async ({
  page,
}) => {
  await page.goto("/live");
  await connected(page);
  const email = page.locator("#signup_email");

  await email.fill(hostile);

  await expect(page.locator("#signup_email_error")).toBeVisible();
  await expect(email).toHaveValue(hostile);
  await expect(page.locator("#injected")).toHaveCount(0);
  expect(await page.evaluate(() => window.injected)).toBeUndefined();
  // The attribute as the server wrote it holds the text, not markup.
  expect(await email.getAttribute("value")).toBe(hostile);
});

test("a checked topic is kept, and a list of topics reaches the Changeset", async ({ page }) => {
  await page.goto("/live");
  await connected(page);
  const username = unique("lister");

  await page.locator("#signup_email").fill("lister@example.com");
  await page.locator("#signup_username").fill(username);
  await page.getByLabel("rust").check();
  await page.getByLabel("elixir").check();
  await expect(page.getByLabel("rust")).toBeChecked();
  await page.getByRole("button", { name: "Sign up" }).click();

  await expect(page.locator("#welcome")).toHaveText(`Welcome, ${username} (rust, elixir)`);
});

test("a submit runs the Context, and its error appears on the right field", async ({ page }) => {
  const frames = formFrames(page);
  await page.goto("/live");
  await connected(page);

  await page.locator("#signup_email").fill("someone@example.com");
  await page.locator("#signup_username").fill("ada");
  // Wait until the server has answered the change that carries `ada`.
  await expect
    .poll(
      () =>
        frames.some((body) => new URLSearchParams(body).get("signup[username]") === "ada") &&
        frames.replied >= frames.length,
    )
    .toBe(true);
  // The change is processed, and the Context does not run on a change: nothing says
  // `ada` is taken.
  await expect(page.locator("#signup_username_error")).toHaveCount(0);
  await page.getByRole("button", { name: "Sign up" }).click();

  await expect(page.locator("#signup_username_error")).toHaveText("has already been taken");
  await expect(page.locator("#signup_email_error")).toHaveCount(0);
  await expect(page.locator("#welcome")).toHaveCount(0);
  await expect(page.locator("#signup_username")).toHaveValue("ada");
});

test("the submit button is disabled with other text while the submit is in flight", async ({
  page,
}) => {
  await page.goto("/live");
  await connected(page);
  const username = unique("hold");
  await page.locator("#signup_email").fill("holder@example.com");
  await page.locator("#signup_username").fill(username);
  const button = page.locator("#signup-form button");
  await expect(button).toHaveText("Sign up");

  // The handler of this username waits at the server until the gate is opened.
  await button.click();
  await expect(button).toBeDisabled();
  await expect(button).toHaveText("Saving...");
  await expect(page.locator("#welcome")).toHaveCount(0);

  const opened = await page.request.get("/test/gate");
  expect(opened.ok()).toBe(true);

  await expect(page.locator("#welcome")).toHaveText(`Welcome, ${username} ()`);
  await expect(button).toBeEnabled();
  await expect(button).toHaveText("Sign up");
});

test("after a reconnect the client sends the form again and typed input is recovered", async ({
  page,
}) => {
  const frames = formFrames(page);
  await page.goto("/live");
  await connected(page);
  await page.locator("#signup_email").fill("ada");
  await page.locator("#signup_username").fill("grace");
  await expect(page.locator("#signup_email_error")).toBeVisible();
  const before = frames.length;

  // The browser's WebSocket is closed under the client, as a dropped connection looks to
  // it. The server's state went with the socket.
  await page.evaluate(() => window.liveSocket.socket.conn.close());

  // The client rejoined and, because the form has an id and a `phx-change`, sent what
  // is in it as a change: every field, as the user typed it.
  await expect.poll(() => frames.length).toBeGreaterThan(before);
  const recovered = new URLSearchParams(frames.at(-1));
  expect(recovered.get("signup[email]")).toBe("ada");
  expect(recovered.get("signup[username]")).toBe("grace");
  await connected(page);
  await expect(page.locator("#signup_email")).toHaveValue("ada");
  await expect(page.locator("#signup_username")).toHaveValue("grace");
  await expect(page.locator("#signup_email_error")).toContainText("must be an address");
});

test("a controller form post without the CSRF token is refused", async ({ page }) => {
  await page.goto("/classic");

  const refused = await page.request.post("/classic", {
    form: { "signup[email]": "a@example.com", "signup[username]": unique("nocsrf") },
  });

  expect(refused.status()).toBe(403);
});

test("a controller form re-renders with errors and what was typed", async ({ page }) => {
  await page.goto("/classic");

  await page.locator("#signup_email").fill(hostile);
  await page.locator("#signup_username").fill("ada");
  await page.getByRole("button", { name: "Sign up" }).click();

  await expect(page.locator("#signup_email_error")).toContainText("must be an address");
  await expect(page.locator("#signup_email")).toHaveValue(hostile);
  await expect(page.locator("#injected")).toHaveCount(0);
  expect(await page.evaluate(() => window.injected)).toBeUndefined();

  // The username is fine to the Changeset, but taken, which the Context finds out.
  await page.locator("#signup_email").fill("someone@example.com");
  await page.getByRole("button", { name: "Sign up" }).click();
  await expect(page.locator("#signup_username_error")).toHaveText("has already been taken");

  // The page is answered 422, which the browser's console reports for a document: the
  // one complaint this test expects, twice, once for each post.
  const status422 = /status of 422/;
  expect(complaints.filter((complaint) => status422.test(complaint))).toHaveLength(2);
  complaints = complaints.filter((complaint) => !status422.test(complaint));
});

test("a controller form that passes redirects with a flash, shown once", async ({ page }) => {
  await page.goto("/classic");
  const username = unique("classic");

  await page.locator("#signup_email").fill("classic@example.com");
  await page.locator("#signup_username").fill(username);
  await page.getByLabel("typescript").check();
  await page.getByRole("button", { name: "Sign up" }).click();

  await expect(page).toHaveURL(/\/classic$/);
  await expect(page.locator(".flash")).toHaveText(`Welcome, ${username} (typescript)`);
  await page.reload();
  await expect(page.locator(".flash")).toHaveCount(0);
});

// The client marks the LiveView's container once the Connected render has arrived.
async function connected(page) {
  await expect(page.locator("[data-phx-main]")).toHaveClass(/phx-connected/);
}

// The URL-encoded body of each form Event the browser sent, in order. Call it before
// the page is loaded.
// `replied` counts the replies to Events (not to the join) the server sent.
function formFrames(page) {
  const bodies = [];
  bodies.replied = 0;
  page.on("websocket", (socket) => {
    socket.on("framereceived", ({ payload }) => {
      const [, , topic, kind, body] = JSON.parse(payload);
      if (topic.startsWith("lv:") && kind === "phx_reply" && !body.response?.rendered) {
        bodies.replied += 1;
      }
    });
    socket.on("framesent", ({ payload }) => {
      const [, , topic, kind, body] = JSON.parse(payload);
      if (topic.startsWith("lv:") && kind === "event" && body.type === "form") {
        bodies.push(body.value);
      }
    });
  });
  return bodies;
}
