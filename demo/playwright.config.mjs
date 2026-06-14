import { defineConfig, devices } from "@playwright/test";

// Smoke-test config for the static demo. By default Playwright starts a local
// static server for ./site and points the test at it; an already-running server
// on the same port is reused (reuseExistingServer). Override the target with
// DEMO_BASE if you serve elsewhere.
const PORT = 8080;
const BASE = process.env.DEMO_BASE || `http://localhost:${PORT}`;

export default defineConfig({
  testDir: "./tests",
  testMatch: "**/*.spec.mjs", // node *.test.mjs files are not Playwright tests
  timeout: 45000,
  fullyParallel: false,
  reporter: "list",
  use: {
    baseURL: BASE,
    headless: true,
  },
  projects: [
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
        // Headless WebGL stability in containers/CI: --disable-dev-shm-usage
        // avoids /dev/shm crashes ("browser has been closed"); swiftshader gives
        // a software GL context for the Three.js waterfall.
        launchOptions: {
          args: [
            "--disable-dev-shm-usage",
            "--no-sandbox",
            "--enable-unsafe-swiftshader",
          ],
        },
      },
    },
  ],
  webServer: {
    command: `python3 -m http.server ${PORT} --directory site --bind 127.0.0.1`,
    url: BASE,
    reuseExistingServer: true,
    timeout: 30000,
  },
});
