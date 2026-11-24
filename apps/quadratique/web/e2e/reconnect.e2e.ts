import type { Page } from "@playwright/test";

import { expect, test } from "./workspace.ts";

const row = (page: Page, status: string, issue: string) =>
  page.getByRole("list", { name: status }).getByRole("listitem").filter({ hasText: issue });

test("the open list reconnects after the server restarts at the same address, and shows what changed while it was down", async ({
  page,
  workspace,
}) => {
  const { repository } = workspace;
  const port = Number(workspace.address.split(":").at(-1));

  await test.step("seed a project with an issue, and open its list", async () => {
    workspace.quadratique(
      "human",
      repository,
      "project",
      "create",
      "--key",
      "DEM",
      "--name",
      "Demo",
    );
    workspace.quadratique(
      "human",
      repository,
      "create",
      "--title",
      "Serve the web app",
      "--acceptance",
      "It is done.",
    );
    await page.goto(workspace.url());
    await expect(page.getByRole("status")).toHaveText("Live");
    await expect(row(page, "Todo", "DEM-1")).toBeVisible();
  });

  let loads = 0;
  page.on("load", () => {
    loads += 1;
  });

  await test.step("the server stops, and the page says it reconnects", async () => {
    await workspace.stop();
    await expect(page.getByRole("status")).toHaveText("Reconnecting");
  });

  await test.step("while the page's server is down, an issue is created on the same data", async () => {
    await workspace.start();
    workspace.quadratique(
      "human",
      repository,
      "create",
      "--title",
      "Made while the page was away",
      "--acceptance",
      "It is done.",
    );
    await workspace.stop();
  });

  await test.step("the server starts again at the page's address; the page is live and shows the issue made meanwhile", async () => {
    await workspace.start(port);
    await expect(page.getByRole("status")).toHaveText("Live");
    await expect(row(page, "Todo", "DEM-2")).toContainText("Made while the page was away");
  });

  await test.step("a change made after the reconnection appears live", async () => {
    workspace.quadratique(
      "human",
      repository,
      "create",
      "--title",
      "Made after the reconnection",
      "--acceptance",
      "It is done.",
    );
    await expect(row(page, "Todo", "DEM-3")).toContainText("Made after the reconnection");
  });

  expect(loads).toBe(0);
});
