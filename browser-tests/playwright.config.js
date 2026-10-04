import { defineConfig } from "@playwright/test";

// Not 4000, so the suite does not collide with an example someone is running by hand.
// BROWSER_TEST_PORT moves it, so two checkouts can run the suite at the same time.
const port = Number(process.env.BROWSER_TEST_PORT ?? 4107);

export default defineConfig({
  testDir: ".",
  forbidOnly: !!process.env.CI,
  reporter: "list",
  use: { baseURL: `http://127.0.0.1:${port}` },
  webServer: [
    {
      command: "cargo run -p counter",
      cwd: "..",
      env: { PORT: String(port) },
      url: `http://127.0.0.1:${port}/`,
      // A first build compiles the workspace.
      timeout: 300_000,
    },
    // The clock example, one port up: `clock.spec.js` sets its own `baseURL`.
    {
      command: "cargo run -p clock",
      cwd: "..",
      env: { PORT: String(port + 1) },
      url: `http://127.0.0.1:${port + 1}/`,
      timeout: 300_000,
    },
    // The commands example, two ports up: `commands.spec.js` sets its own `baseURL`.
    {
      command: "cargo run -p commands",
      cwd: "..",
      env: { PORT: String(port + 2) },
      url: `http://127.0.0.1:${port + 2}/`,
      timeout: 300_000,
    },
    // The forms example, four ports up: `forms.spec.js` sets its own `baseURL`. The
    // gate lets a test hold a submit in flight and release it.
    {
      command: "cargo run -p forms",
      cwd: "..",
      env: { PORT: String(port + 4), FORMS_GATE: "1" },
      url: `http://127.0.0.1:${port + 4}/classic`,
      timeout: 300_000,
    },
    // The navigation example, three ports up: `navigation.spec.js` sets its own `baseURL`.
    {
      command: "cargo run -p navigation",
      cwd: "..",
      env: { PORT: String(port + 3) },
      url: `http://127.0.0.1:${port + 3}/`,
      timeout: 300_000,
    },
  ],
});
