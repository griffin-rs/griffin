// The navigation example: patching the URL, live navigation, a redirect with flash and
// the page title, through the unmodified Phoenix client in a real browser.
//
// Whether the page was loaded again is read from what a load destroys: `window.marker`,
// which a test sets on the page, and the number of document requests.

import { expect, test } from "@playwright/test";

// The example's server: three ports up from the counter's (`playwright.config.js`).
const port = Number(process.env.BROWSER_TEST_PORT ?? 4107) + 3;
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

test("a patch link changes the URL and the page, keeps the state and loads nothing", async ({
  page,
}) => {
  const traffic = watch(page);
  await page.goto("/");
  await connected(page);
  await expect(page.locator("#range")).toHaveText("1-5");
  await expect(page).toHaveTitle("Items, page 1");
  await page.locator("#count").click();
  await expect(page.locator("#clicks")).toHaveText("1");
  await mark(page);

  await page.locator("#page-2").click();

  await expect(page).toHaveURL("/?page=2");
  await expect(page.locator("#range")).toHaveText("6-10");
  await expect(page).toHaveTitle("Items, page 2");
  // The state is kept: the LiveView was not mounted again, and the page was not loaded.
  await expect(page.locator("#clicks")).toHaveText("1");
  await expectNotLoadedAgain(page);
  expect(traffic.documents).toEqual(["/"]);
  expect(traffic.joins).toBe(1);
  expect(traffic.sent).toEqual(["event", "live_patch"]);
});

test("a patch from the server changes the URL as a link does", async ({ page }) => {
  const traffic = watch(page);
  await page.goto("/");
  await connected(page);
  await mark(page);

  await page.locator("#next").click();

  await expect(page).toHaveURL("/?page=2");
  await expect(page.locator("#range")).toHaveText("6-10");
  await expect(page).toHaveTitle("Items, page 2");
  await expectNotLoadedAgain(page);
  expect(traffic.documents).toEqual(["/"]);
  expect(traffic.joins).toBe(1);
});

test("the Dead render of a patched URL has the page and the title", async ({ page }) => {
  const response = await page.goto("/?page=3");

  const html = await response.text();
  expect(html).toContain('<output id="range">11-12</output>');
  expect(html).toContain("<title>Items, page 3</title>");
});

test("back and forward walk across patches without loading the page", async ({ page }) => {
  await page.goto("/");
  await connected(page);
  await mark(page);
  await page.locator("#page-2").click();
  await expect(page).toHaveURL("/?page=2");
  await page.locator("#page-3").click();
  await expect(page).toHaveURL("/?page=3");
  await expect(page.locator("#range")).toHaveText("11-12");

  await page.goBack();
  await expect(page).toHaveURL("/?page=2");
  await expect(page.locator("#range")).toHaveText("6-10");
  await expect(page).toHaveTitle("Items, page 2");
  await page.goBack();
  await expect(page).toHaveURL("/");
  await expect(page.locator("#range")).toHaveText("1-5");

  await page.goForward();
  await expect(page).toHaveURL("/?page=2");
  await expect(page.locator("#range")).toHaveText("6-10");
  await page.goForward();
  await expect(page).toHaveURL("/?page=3");
  await expect(page.locator("#range")).toHaveText("11-12");
  await expectNotLoadedAgain(page);
});

test("a navigation to a LiveView of the same live session swaps the page over the open connection", async ({
  page,
}) => {
  const traffic = watch(page);
  await page.goto("/");
  await connected(page);
  await mark(page);

  await page.locator("#about-link").click();

  await expect(page).toHaveURL("/about");
  await expect(page.locator("h1")).toHaveText("About");
  await expect(page).toHaveTitle("About");
  // No full load: the script's state survived, there was one document request, and
  // the socket was not opened again.
  await expectNotLoadedAgain(page);
  expect(traffic.documents).toEqual(["/"]);
  expect(traffic.sockets).toBe(1);
  // The second LiveView was joined as a navigation.
  expect(traffic.joins).toBe(2);

  // And back, the same way.
  await page.locator("#items-link").click();
  await expect(page).toHaveURL("/");
  await expect(page.locator("h1")).toHaveText("Items");
  await expectNotLoadedAgain(page);
  expect(traffic.documents).toEqual(["/"]);
});

test("back and forward work across a navigation too", async ({ page }) => {
  await page.goto("/");
  await connected(page);
  await mark(page);
  await page.locator("#about-link").click();
  await expect(page.locator("h1")).toHaveText("About");

  await page.goBack();
  await expect(page).toHaveURL("/");
  await expect(page.locator("h1")).toHaveText("Items");
  await page.goForward();
  await expect(page).toHaveURL("/about");
  await expect(page.locator("h1")).toHaveText("About");
  await expectNotLoadedAgain(page);
});

test("a navigation across live sessions loads the page", async ({ page }) => {
  const traffic = watch(page);
  await page.goto("/");
  await connected(page);
  await mark(page);

  await page.locator("#admin-link").click();

  await expect(page).toHaveURL("/admin");
  await expect(page.locator("h1")).toHaveText("Admin");
  await expect(page).toHaveTitle("Admin");
  // The page was loaded: what the first page's script held is gone, and the second
  // document was asked for from the server.
  await expect.poll(() => page.evaluate(() => window.marker)).toBeUndefined();
  expect(traffic.documents).toEqual(["/", "/admin"]);
});

test("a redirect from a LiveView carries a flash that the next page shows once", async ({
  page,
}) => {
  const traffic = watch(page);
  await page.goto("/");
  await connected(page);
  await mark(page);

  await page.locator("#save").click();

  await expect(page).toHaveURL("/goodbye");
  await expect(page.locator("h1")).toHaveText("Goodbye");
  await expect(page.locator("#flash")).toHaveText("Saved");
  // A page that is not live is loaded.
  await expect.poll(() => page.evaluate(() => window.marker)).toBeUndefined();
  expect(traffic.documents).toEqual(["/", "/goodbye"]);

  // Shown once: the same page again has none.
  await page.reload();
  await expect(page.locator("h1")).toHaveText("Goodbye");
  await expect(page.locator("#flash")).toHaveCount(0);
});

test("a flash cookie nobody signed shows nothing", async ({ page, context }) => {
  await context.addCookies([
    {
      name: "__phoenix_flash__",
      value: "forged.token",
      url: `http://127.0.0.1:${port}/`,
    },
  ]);

  await page.goto("/goodbye");

  await expect(page.locator("h1")).toHaveText("Goodbye");
  await expect(page.locator("#flash")).toHaveCount(0);
});

// What the server was sent and asked for. A document request is a load of a page.
function watch(page) {
  const traffic = { documents: [], sent: [], joins: 0, sockets: 0 };
  page.on("request", (request) => {
    if (request.resourceType() === "document") {
      traffic.documents.push(new URL(request.url()).pathname);
    }
  });
  page.on("websocket", (socket) => {
    traffic.sockets += 1;
    socket.on("framesent", ({ payload }) => {
      const [, , topic, kind] = JSON.parse(payload);
      if (!topic.startsWith("lv:")) return;
      if (kind === "phx_join") traffic.joins += 1;
      else if (kind !== "phx_leave") traffic.sent.push(kind);
    });
  });
  return traffic;
}

// What only the page that is open now has: a load of another page loses it.
async function mark(page) {
  await page.evaluate(() => {
    window.marker = "this page, not loaded again";
  });
}

async function expectNotLoadedAgain(page) {
  expect(await page.evaluate(() => window.marker)).toBe("this page, not loaded again");
}

// The client marks the LiveView's container once the Connected render has arrived.
async function connected(page) {
  await expect(page.locator("[data-phx-main]")).toHaveClass(/phx-connected/);
}
