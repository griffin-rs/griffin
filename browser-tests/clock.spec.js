// The clock example: a page that a timer on the server updates, through the unmodified
// Phoenix client in a real browser. Nothing is clicked or typed in this file.

import { expect, test } from "@playwright/test";

// The example's server: one port up from the counter's (`playwright.config.js`).
const port = Number(process.env.BROWSER_TEST_PORT ?? 4107) + 1;
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

test("a timer-driven page updates without interaction", async ({ page }) => {
  // What goes over the socket for the LiveView, besides the join and its reply.
  const sent = [];
  const pushed = [];
  page.on("websocket", (socket) => {
    socket.on("framesent", ({ payload }) => {
      const [, , topic, kind] = JSON.parse(payload);
      if (topic.startsWith("lv:") && kind !== "phx_join") sent.push(kind);
    });
    socket.on("framereceived", ({ payload }) => {
      const [, ref, topic, kind, body] = JSON.parse(payload);
      if (topic.startsWith("lv:") && kind !== "phx_reply") pushed.push({ ref, kind, body });
    });
  });

  // The Dead render starts no timer: the page arrives at zero.
  const response = await page.goto("/");
  expect(await response.text()).toContain('<output id="ticks">0</output>');
  const main = page.locator("[data-phx-main]");
  await expect(main).toHaveClass(/phx-connected/);
  const container = await main.getAttribute("id");

  // The number goes up by itself.
  const ticks = page.locator("#ticks");
  await expect.poll(async () => Number(await ticks.textContent())).toBeGreaterThanOrEqual(3);

  // It went up by diffs the server pushed: each is a `diff` with no ref, as it answers
  // nothing, and they came in order, one tick each.
  expect(pushed.length).toBeGreaterThanOrEqual(3);
  expect(pushed.map(({ ref, kind }) => [ref, kind])).toEqual(pushed.map(() => [null, "diff"]));
  expect(pushed.map(({ body }) => Number(body["0"]))).toEqual(pushed.map((_, index) => index + 1));
  // The browser asked for none of it,
  expect(sent).toEqual([]);
  // and the page was patched, not loaded again.
  await expect(main).toHaveAttribute("id", container);
});
