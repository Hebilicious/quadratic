import { fileURLToPath, URL } from "node:url";

import { defineConfig, devices } from "@playwright/test";

import { HANG_GUARD_MS } from "./hang-guard.ts";

export default defineConfig({
  expect: { timeout: HANG_GUARD_MS },
  forbidOnly: true,
  fullyParallel: false,
  outputDir: fileURLToPath(new URL("../test-results", import.meta.url)),
  projects: [
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
        launchOptions: { args: ["--enable-features=WebMCP"] },
      },
    },
  ],
  reporter: [["list"]],
  retries: 0,
  testDir: ".",
  testMatch: "**/*.e2e.ts",
  timeout: 5 * 60_000,
  use: {
    actionTimeout: HANG_GUARD_MS,
    navigationTimeout: HANG_GUARD_MS,
    trace: "retain-on-failure",
  },
  workers: 1,
});
