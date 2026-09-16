import { defineConfig } from "@playwright/test";

const port = process.env.BROWSER_TEST_PORT ?? "3100";
const baseURL = `http://127.0.0.1:${port}`;

export default defineConfig({
  testDir: "tests/browser",
  fullyParallel: false,
  retries: process.env.CI ? 2 : 0,
  reporter: process.env.CI ? "github" : "list",
  use: {
    baseURL,
    browserName: "chromium",
    trace: "retain-on-failure",
  },
  webServer: {
    command: "./scripts/run-browser-test-server.sh",
    env: {
      ...process.env,
      ADMIN_LISTEN_ADDRESS: `127.0.0.1:${port}`,
      GITHUB_API_BASE_URL: "http://127.0.0.1:3101",
      GITHUB_WEBHOOK_SECRET: "browser-test-github-webhook-secret-2026",
    },
    url: `${baseURL}/health/ready`,
    reuseExistingServer: false,
    timeout: 120_000,
    stdout: "pipe",
    stderr: "pipe",
  },
  globalSetup: "./tests/browser/global-setup.ts",
});
