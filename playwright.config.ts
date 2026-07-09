import { defineConfig, devices } from "@playwright/test";

// Frontend-only browser smoke suite. tauri-driver (the WebDriver bridge for
// driving the real Tauri window) does not support macOS, so this exercises
// the Svelte app in a real Chromium browser against the Vite dev server,
// with the Tauri IPC layer stubbed (see tests/e2e/tauri-stub.ts) instead of
// a real Rust backend.
export default defineConfig({
  testDir: "./tests/e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? "line" : "list",
  use: {
    baseURL: "http://localhost:1420",
    trace: "on-first-retry",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
  webServer: {
    command: "npm run dev",
    url: "http://localhost:1420",
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
  },
});
