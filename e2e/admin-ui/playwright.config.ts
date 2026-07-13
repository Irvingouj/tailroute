import { defineConfig } from "@playwright/test";
import path from "path";

const port = process.env.ADMIN_E2E_PORT || "18081";
const baseURL = process.env.ADMIN_E2E_BASE_URL || `http://127.0.0.1:${port}`;

export default defineConfig({
  testDir: "./tests",
  timeout: 120_000,
  expect: { timeout: 15_000 },
  fullyParallel: false,
  workers: 1,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [["github"], ["list"]] : "list",
  use: {
    baseURL,
    trace: "on-first-retry",
    screenshot: "only-on-failure",
  },
  webServer: process.env.ADMIN_E2E_BASE_URL
    ? undefined
    : {
        command: `bash ${path.join(__dirname, "scripts/start-controller.sh")} ${port}`,
        url: `${baseURL}/health`,
        reuseExistingServer: !process.env.CI,
        timeout: 180_000,
        stdout: "pipe",
        stderr: "pipe",
      },
});
