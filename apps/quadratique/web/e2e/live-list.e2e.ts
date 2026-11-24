import type { Page } from "@playwright/test";

import { expect, test } from "./workspace.ts";

const group = (page: Page, status: string) => page.getByRole("list", { name: status });
const row = (page: Page, status: string, issue: string) =>
  group(page, status).getByRole("listitem").filter({ hasText: issue });

test("the human watches agents work on the live list, with no reload", async ({
  page,
  workspace,
}) => {
  const { repository } = workspace;

  await test.step("seed a project and its issues from the command line", () => {
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
      "--priority",
      "2",
    );
    workspace.quadratique(
      "human",
      repository,
      "create",
      "--title",
      "Show every claim",
      "--acceptance",
      "It is done.",
    );
    workspace.quadratique(
      "human",
      repository,
      "create",
      "--title",
      "Watch the refs",
      "--acceptance",
      "It is done.",
      "--status",
      "backlog",
    );
  });

  let loads = 0;
  page.on("load", () => {
    loads += 1;
  });

  await test.step("open the UI and see the issues grouped by status", async () => {
    await page.goto(workspace.url());
    await expect(page).toHaveURL(/\/projects\/DEM$/u);
    await expect(page.getByRole("status")).toHaveText("Live");
    await expect(row(page, "Todo", "DEM-1")).toContainText("Serve the web app");
    await expect(row(page, "Todo", "DEM-2")).toContainText("Show every claim");
    await expect(row(page, "Backlog", "DEM-3")).toContainText("Watch the refs");
  });

  await test.step("an agent in a worktree claims an issue, and its row moves and shows the claim", async () => {
    const worktree = await workspace.worktree("agent-2", "dem-2-claims");
    workspace.quadratique("agent", worktree, "claim", "--issue", "DEM-2");

    const claimed = row(page, "In progress", "DEM-2");
    await expect(claimed).toContainText("claude-code@agent-2");
    await expect(claimed).toContainText("dem-2-claims");
    await expect(row(page, "Todo", "DEM-2")).toHaveCount(0);
  });

  await test.step("an issue created from the command line appears", async () => {
    workspace.quadratique(
      "human",
      repository,
      "create",
      "--title",
      "Plan the now view",
      "--acceptance",
      "It is done.",
    );
    await expect(row(page, "Todo", "DEM-4")).toContainText("Plan the now view");
  });

  await test.step("an agent in the browser files an issue through WebMCP, as the agent browser", async () => {
    const answer = await page.evaluate(async () => {
      const context = document.modelContext;
      const tool = (await context?.getTools())?.find((each) => each.name === "issue.create");
      if (context === undefined || tool === undefined) {
        throw new Error("issue.create is not registered as a WebMCP tool");
      }
      return await context.executeTool(tool, {
        project: "DEM",
        title: "Filed from the browser",
        acceptance: "It is done.",
      });
    });
    expect(JSON.parse(answer)).toMatchObject({ ok: true, value: { issue: { id: "DEM-5" } } });
    await expect(row(page, "Todo", "DEM-5")).toContainText("Filed from the browser");
    const filed = workspace.quadratique("human", repository, "show", "--issue", "DEM-5");
    expect(filed).toMatchObject({ issue: { createdBy: { kind: "agent", name: "browser" } } });
  });

  expect(loads).toBe(1);
});
