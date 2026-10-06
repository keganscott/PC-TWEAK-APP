import { defineConfig, devices } from "@playwright/test";

// Smoke test of the UI against the SAMPLE mock (plan section 7). Runs the
// release bundle built with VITE_SAMPLE=1, served by `vite preview`.
// PW_CHROMIUM_PATH lets a machine with a preinstalled Chromium skip the download.
const executablePath = process.env.PW_CHROMIUM_PATH || undefined;

export default defineConfig({
  testDir: "e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL: "http://localhost:4173",
    trace: "retain-on-failure",
    launchOptions: executablePath ? { executablePath } : {},
  },
  projects: [
    { name: "1366x768", use: { ...devices["Desktop Chrome"], viewport: { width: 1366, height: 768 } } },
    { name: "1366x768 at 150%", use: { ...devices["Desktop Chrome"], viewport: { width: 911, height: 512 }, deviceScaleFactor: 1.5 } },
  ],
  webServer: {
    command: "npx vite preview --outDir dist-e2e --port 4173 --strictPort",
    url: "http://localhost:4173",
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
  },
});
