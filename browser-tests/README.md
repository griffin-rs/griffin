# Browser tests

A headless browser loads an example app and uses it through the unmodified Phoenix client: the end-to-end proof that Griffin speaks Phoenix's wire protocol unchanged. Handshake, the order of updates and reconnecting are checked here, where the real client is the judge.

- `counter.spec.js`: the counter example (`examples/counter`).
- `clock.spec.js`: the clock example (`examples/clock`): a page that a timer on the server updates. Nothing is clicked; the test reads the frames on the WebSocket to see that the diffs were pushed and none was asked for.
- `commands.spec.js`: the commands example (`examples/commands`): client commands, hooks, pushed events, replies, and key, window, focus, click-away, debounce and throttle bindings, and focus kept across a patch. It reads the frames on the WebSocket where a test is about what is sent, or not sent.
- `forms.spec.js`: the forms example (`examples/forms`): a live form and a controller form over one Changeset: errors only for touched fields, a contextual error from the Context, the submit button while the submit is in flight (held at the server by a gate the test opens), form recovery after a reconnect, and the CSRF token of the controller form.
- `navigation.spec.js`: the navigation example (`examples/navigation`): patching the URL, live navigation within a live session and the full page load across two, back and forward, a redirect with flash and the page title. A page that was not loaded again is told by what a load destroys: a marker the test sets on `window`, and the document requests.
- `playwright.config.js`: builds and starts each example with `cargo run -p <example>`, the counter on port 4107, the clock one port up, the commands example two ports up, the navigation example three ports up and the forms example four ports up, and stops them afterwards. `BROWSER_TEST_PORT` moves the first port.

This is the only part of Griffin that needs Node, and only to develop Griffin itself: `cargo test --workspace` does not run these tests and needs no Node, and an application built with Griffin never needs it. The one dependency is [Playwright](https://playwright.dev), which drives the browser.

Set up once (Node 20 or later; the browser goes to Playwright's cache in your home directory, not into the repository):

```bash
npm ci --prefix browser-tests
npx --prefix browser-tests playwright install chromium
```

Run, from the repository root:

```bash
npm test --prefix browser-tests
```

## Adding a case

1. Give the example what the case needs, or add an example under `examples/` and a `webServer` entry for it in `playwright.config.js`.
2. Add a test to the example's spec file. Wait for the Connected render (`connected(page)`) before the first click, and wait on what the page shows (`await expect(locator).toHaveText(..)`), never on time.
3. Every test in a spec file also fails if the browser console shows an error or a warning. A test that expects one must say so itself.

To watch a test run, add `-- --headed` or `-- --ui` to the command.
