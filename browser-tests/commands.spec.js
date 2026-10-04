// The commands example: client commands, client hooks, pushed events, replies and the
// client's own bindings, through the unmodified Phoenix client in a real browser.

import { expect, test } from "@playwright/test";

// The example's server: two ports up from the counter's (`playwright.config.js`).
const port = Number(process.env.BROWSER_TEST_PORT ?? 4107) + 2;
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

test("a command toggles an element in the browser and sends no frame", async ({ page }) => {
  const frames = liveViewFrames(page);
  await page.goto("/");
  const menu = page.locator("#menu");
  await connected(page);
  await expect(menu).toBeHidden();

  await page.locator("#toggle").click();
  await expect(menu).toBeVisible();
  await page.locator("#toggle").click();
  await expect(menu).toBeHidden();

  // Frames leave in order. Once a later Event has been answered, a frame either
  // toggle had sent would be among the sent ones, before it. There is only the Event.
  await page.locator("#highlight").click();
  await expect(page.locator("#highlighted")).toHaveText("item-2");
  expect(frames.sent.map((frame) => [frame.kind, frame.payload.event])).toEqual([
    ["event", "highlight"],
  ]);
});

test("a chain of commands toggles and pushes a typed Event", async ({ page }) => {
  const frames = liveViewFrames(page);
  await page.goto("/");
  const menu = page.locator("#menu");
  const button = page.locator("#toggle-and-tell");
  await connected(page);
  // The Event's field is in the command, not in a `phx-value-*` attribute.
  await expect(button).not.toHaveAttribute("phx-value-by");

  await button.click();

  // The server handled the Event the chain pushed, with its field,
  await expect(page.locator("#told")).toHaveText("1");
  await expect(page.locator("#told-by")).toHaveText("the second button");
  expect(frames.sent.map((frame) => frame.payload)).toMatchObject([
    { type: "click", event: "toggled", value: { by: "the second button" } },
  ]);
  // and what the commands before it did in the browser outlives the patch.
  await expect(menu).toBeVisible();
  await expect(button).toHaveClass("told");

  await button.click();
  await expect(page.locator("#told")).toHaveText("2");
  await expect(menu).toBeHidden();
});

test("a hook pushes an Event and is given the handler's reply", async ({ page }) => {
  const frames = liveViewFrames(page);
  await page.goto("/");
  await connected(page);

  await page.locator("#lookup").click();

  // What the hook made of the reply: JSON, so the text is as the handler wrote it.
  await expect(page.locator("#lookup-result")).toHaveText('3: Milk & "eggs" <fresh>');
  expect(frames.sent.map((frame) => frame.payload)).toMatchObject([
    { type: "hook", event: "lookup", value: { id: 3 } },
  ]);
  expect(frames.replies).toEqual([
    { diff: { r: { id: 3, name: 'Milk & "eggs" <fresh>' } } },
  ]);
});

test("an event the server pushes reaches a hook's handler", async ({ page }) => {
  const frames = liveViewFrames(page);
  await page.goto("/");
  await connected(page);
  await expect(page.locator("#highlighted")).toHaveText("");

  await page.locator("#highlight").click();

  await expect(page.locator("#highlighted")).toHaveText("item-2");
  expect(frames.replies).toEqual([{ diff: { e: [["highlight", { id: "item-2" }]] } }]);
});

test("a debounced input sends fewer Events than keystrokes", async ({ page }) => {
  const frames = liveViewFrames(page);
  await page.goto("/bindings");
  await connected(page);

  await page.locator("#search").pressSequentially("hello");

  // The page shows what the last Event carried, so every Event has been sent by now.
  await expect(page.locator("#searched")).toHaveText("hello");
  const sent = (event) => frames.sent.filter((frame) => frame.payload.event === event);
  // Five keystrokes: the window's key binding, which is not debounced, sent each one,
  expect(sent("key_down").map((frame) => frame.payload.value.key)).toEqual([..."hello"]);
  // and the input's debounced one waited for the typing to pause: fewer Events than
  // keystrokes, the last with the whole text.
  const searches = sent("search").map((frame) => frame.payload);
  expect(searches.length).toBeLessThan(5);
  expect(searches.at(-1)).toMatchObject({ type: "keyup", value: { key: "o", value: "hello" } });
});

test("a throttled input sends the first Event and holds back the rest", async ({ page }) => {
  const frames = liveViewFrames(page);
  await page.goto("/bindings");
  await connected(page);

  await page.locator("#throttled").pressSequentially("hello");

  // The window is long, so only the first keystroke got through; it is the one shown.
  await expect(page.locator("#searched")).toHaveText("h");
  const searches = frames.sent.filter((frame) => frame.payload.event === "search");
  expect(searches.map((frame) => frame.payload.value.key)).toEqual(["h"]);
});

test("a patch keeps the focus and what was typed", async ({ page }) => {
  await page.goto("/bindings");
  await connected(page);
  const search = page.locator("#search");
  await search.focus();

  await search.pressSequentially("hello");

  // The debounced Event changed the page around the input; the input is the same one.
  await expect(page.locator("#searched")).toHaveText("hello");
  await expect(search).toBeFocused();
  await expect(search).toHaveValue("hello");
});

test("key bindings on the window deliver the key", async ({ page }) => {
  const frames = liveViewFrames(page);
  await page.goto("/bindings");
  await connected(page);

  await page.keyboard.press("k");
  await expect(page.locator("#last-key")).toHaveText("k");
  await page.keyboard.press("Escape");
  await expect(page.locator("#escapes")).toHaveText("1");
  await expect(page.locator("#last-key")).toHaveText("Escape");

  // `phx-key` let only Escape through to the key-up binding.
  expect(frames.sent.map((frame) => frame.payload)).toMatchObject([
    { type: "keydown", event: "key_down", value: { key: "k" } },
    { type: "keydown", event: "key_down", value: { key: "Escape" } },
    { type: "keyup", event: "escaped", value: { key: "Escape" } },
  ]);
});

test("focus, blur and click-away bindings deliver their Events", async ({ page }) => {
  await page.goto("/bindings");
  const search = page.locator("#search");
  const focus = page.locator("#focus");
  await connected(page);
  await expect(focus).toHaveText("nowhere");

  await search.focus();
  await expect(focus).toHaveText("in the search");
  await search.pressSequentially("ab");
  await expect(page.locator("#searched")).toHaveText("ab");

  // A click on the panel takes the focus from the input, whose value goes with the
  // blur, and is not a click away from the panel.
  await page.locator("#panel").click();
  await expect(focus).toHaveText("left with ab");
  // A click elsewhere is. The count is 1: the click on the panel was not counted.
  await page.locator("h1").click();
  await expect(page.locator("#away")).toHaveText("1");
});

// What goes over the socket for the page's LiveView after its join, in order: `sent`
// is each frame from the browser, as `{kind, payload}`, where an Event's kind is
// `event` and its payload the client's `{type, event, value}`; `replies` is the
// response of each reply from the server. Call it before the page is loaded.
function liveViewFrames(page) {
  const frames = { sent: [], replies: [] };
  page.on("websocket", (socket) => {
    socket.on("framesent", ({ payload }) => {
      const [, , topic, kind, body] = JSON.parse(payload);
      if (topic.startsWith("lv:") && kind !== "phx_join") {
        frames.sent.push({ kind, payload: body });
      }
    });
    socket.on("framereceived", ({ payload }) => {
      const [, , topic, kind, body] = JSON.parse(payload);
      if (topic.startsWith("lv:") && kind === "phx_reply" && !body.response.rendered) {
        frames.replies.push(body.response);
      }
    });
  });
  return frames;
}

// The client marks the LiveView's container once the Connected render has arrived.
async function connected(page) {
  await expect(page.locator("[data-phx-main]")).toHaveClass(/phx-connected/);
}
