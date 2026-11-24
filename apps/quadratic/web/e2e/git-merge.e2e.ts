import { writeFile } from "node:fs/promises";
import { join } from "node:path";

import type { Page } from "@playwright/test";

import { expect, test } from "./workspace.ts";

const card = (page: Page, status: string, issue: string) =>
  page.getByRole("list", { name: status }).getByRole("listitem").filter({ hasText: issue });

test("a squash merge whose commit says Refs: DEM-1 moves the card to Merged on the open board, with no reload", async ({
  page,
  workspace,
}) => {
  const { repository } = workspace;
  let worktree = "";

  await test.step("an agent claims an issue on a branch whose name does not hold its id, and finishes it", async () => {
    workspace.quadratic("human", repository, "project", "create", "--key", "DEM", "--name", "Demo");
    workspace.quadratic(
      "human",
      repository,
      "create",
      "--title",
      "Write the guide",
      "--acceptance",
      "It is done.",
    );
    worktree = await workspace.worktree("agent-1", "guide-work");
    workspace.quadratic("agent", worktree, "claim", "--issue", "DEM-1");
    await writeFile(join(worktree, "guide.md"), "# The guide\n");
    await workspace.git(worktree, "add", "guide.md");
    await workspace.git(worktree, "commit", "--quiet", "--message", "Write the guide");
    workspace.quadratic(
      "agent",
      worktree,
      "close",
      "--issue",
      "DEM-1",
      "--evidence",
      "guide.md written",
    );
  });

  let loads = 0;
  page.on("load", () => {
    loads += 1;
  });

  await test.step("the board shows the card in review, with its claim", async () => {
    await page.goto(workspace.url("/projects/DEM/board"));
    await expect(card(page, "In review", "DEM-1")).toContainText("guide-work");
  });

  await test.step("the branch is squashed into main with git alone, in one commit that says Refs: DEM-1", async () => {
    await workspace.git(repository, "merge", "--squash", "guide-work");
    await workspace.git(
      repository,
      "commit",
      "--quiet",
      "--message",
      "Write the guide\n\nRefs: DEM-1",
    );
  });

  await test.step("the card moves to Merged by itself", async () => {
    await expect(card(page, "Merged", "DEM-1")).toContainText("Write the guide");
    await expect(card(page, "In review", "DEM-1")).toHaveCount(0);
  });

  expect(loads).toBe(1);
});
