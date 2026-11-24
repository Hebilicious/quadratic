import type { Page } from "@playwright/test";

import { expect, test } from "./workspace.ts";

test.use({ viewport: { width: 1440, height: 900 } });

const card = (page: Page, status: string, issue: string) =>
  page.getByRole("list", { name: status }).getByRole("listitem").filter({ hasText: issue });

test("the human reads who changed what, opens the board as it was after the first change, and comes back to now", async ({
  page,
  workspace,
}) => {
  const { repository } = workspace;

  await test.step("seed a project with two issues, then make three changes from the command line", async () => {
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
      "issue",
      "update",
      "--issue",
      "DEM-1",
      "--revision",
      workspace.revision("DEM-1"),
      "--status",
      "in-progress",
    );
    workspace.quadratique(
      "human",
      repository,
      "issue",
      "update",
      "--issue",
      "DEM-2",
      "--revision",
      workspace.revision("DEM-2"),
      "--priority",
      "1",
    );
    const worktree = await workspace.worktree("agent-2", "dem-2-claims");
    workspace.quadratique("agent", worktree, "claim", "--issue", "DEM-2");
  });

  const changes = (page: Page) =>
    page.getByRole("list", { name: "Changes, newest first" }).locator(":scope > li");

  await test.step("the History link opens the history of the project: the three changes, newest first, in plain words", async () => {
    await page.goto(workspace.url("/projects/DEM"));
    await expect(page.getByRole("status")).toHaveText("Live");
    await page.getByRole("group", { name: "View" }).getByRole("link", { name: "History" }).click();
    await expect(page).toHaveURL(/\/projects\/DEM\/history$/u);
    await expect(changes(page).nth(0)).toContainText(
      "claude-code@agent-2 claimed DEM-2 on dem-2-claims",
    );
    await expect(changes(page).nth(0)).toContainText("Status: Todo → In progress");
    await expect(changes(page).nth(1)).toContainText("tester changed DEM-2");
    await expect(changes(page).nth(1)).toContainText("Priority: No priority → Urgent");
    await expect(changes(page).nth(2)).toContainText("tester changed DEM-1");
    await expect(changes(page).nth(2)).toContainText("Status: Todo → In progress");
  });

  await test.step("Board then, on the first of the three changes, shows the board after it, marked as the past", async () => {
    await changes(page)
      .nth(2)
      .getByRole("link", { name: /The board after this change/u })
      .click();
    await expect(page).toHaveURL(/\/projects\/DEM\/board\?at=[0-9a-f]{7,64}$/u);
    await expect(page.getByRole("note", { name: "The past" })).toContainText(
      "The board as it was at",
    );
    await expect(page.getByRole("status")).not.toHaveText("Live");
    await expect(card(page, "In progress", "DEM-1")).toBeVisible();
    await expect(card(page, "Todo", "DEM-2")).toBeVisible();
    await expect(card(page, "Todo", "DEM-2")).not.toContainText("claude-code@agent-2");
  });

  await test.step("nothing can be changed in the past: no create button, no drag, and the keys that change issues do nothing", async () => {
    await expect(page.getByRole("button", { name: /New issue/u })).toHaveCount(0);
    await expect(card(page, "Todo", "DEM-2")).toHaveAttribute("draggable", "false");
    await page.keyboard.press("j");
    await page.keyboard.press("l");
    await expect(card(page, "In progress", "DEM-1")).toHaveAttribute("aria-current", "true");
    await page.keyboard.press("Shift+L");
    await page.keyboard.press("c");
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  await test.step("Back to now shows the board as it is: DEM-2 claimed and in progress", async () => {
    await page.getByRole("link", { name: "Back to now" }).click();
    await expect(page).toHaveURL(/\/projects\/DEM\/board$/u);
    await expect(page.getByRole("note", { name: "The past" })).toHaveCount(0);
    await expect(card(page, "In progress", "DEM-2")).toContainText("claude-code@agent-2");
    await expect(card(page, "In progress", "DEM-1")).toBeVisible();
  });

  expect(workspace.quadratique("human", repository, "show", "--issue", "DEM-1")).toMatchObject({
    issue: { status: "in-progress" },
  });
});
