import type { Page } from "@playwright/test";

import { expect, test } from "./workspace.ts";

test.use({ viewport: { width: 1440, height: 900 } });

const item = (page: Page, group: string, issue: string) =>
  page
    .getByRole("list", { name: group, exact: true })
    .getByRole("listitem")
    .filter({ hasText: issue });

test("the human groups the board by priority, moves a card to another priority by the keyboard, and groups by label", async ({
  page,
  workspace,
}) => {
  const { repository } = workspace;

  await test.step("seed a project: an urgent issue with two labels, a high one with one, and one with neither", () => {
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
      "1",
      "--labels",
      "web",
      "--labels",
      "server",
    );
    workspace.quadratique(
      "human",
      repository,
      "create",
      "--title",
      "Show every claim",
      "--acceptance",
      "It is done.",
      "--priority",
      "2",
      "--labels",
      "web",
    );
    workspace.quadratique(
      "human",
      repository,
      "create",
      "--title",
      "Watch the refs",
      "--acceptance",
      "It is done.",
    );
  });

  await test.step("the command menu groups the board by priority, and the address holds the grouping", async () => {
    await page.goto(workspace.url("/projects/DEM/board"));
    await expect(page.getByRole("status")).toHaveText("Live");
    await page.keyboard.press("ControlOrMeta+k");
    await page.keyboard.type("group by priority");
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/\/projects\/DEM\/board\?group=priority$/u);
  });

  await test.step("J puts the cursor on the urgent card, and Shift L moves it to the high column", async () => {
    await expect(item(page, "Urgent", "DEM-1")).toBeVisible();
    await page.keyboard.press("j");
    await expect(item(page, "Urgent", "DEM-1")).toHaveAttribute("aria-current", "true");
    await page.keyboard.press("Shift+L");
    await expect(item(page, "High", "DEM-1")).toBeVisible();
    await expect(item(page, "Urgent", "DEM-1")).toHaveCount(0);
  });

  await test.step("Mod B shows the list, still grouped by priority, with the issue under High", async () => {
    await page.keyboard.press("ControlOrMeta+b");
    await expect(page).toHaveURL(/\/projects\/DEM\?group=priority$/u);
    await expect(item(page, "High", "DEM-1")).toBeVisible();
  });

  await test.step("the Group by field groups the list by label, and the address follows", async () => {
    await page.getByLabel("Group by").selectOption("label");
    await expect(page).toHaveURL(/\/projects\/DEM\?group=label$/u);
  });

  await test.step("the issue under two labels is one stop of the cursor: only its first copy is current and reachable by Tab", async () => {
    await expect(item(page, "server", "DEM-1")).toHaveAttribute("aria-current", "true");
    await expect(page.locator('[data-issue][aria-current="true"]')).toHaveCount(1);
    await expect(page.locator('[data-issue][tabindex="0"]')).toHaveCount(1);
  });

  await test.step("on the board grouped by label, a card cannot be dragged: a label is not one value to move", async () => {
    await page.keyboard.press("ControlOrMeta+b");
    await expect(item(page, "web", "DEM-2")).toHaveAttribute("draggable", "false");
  });

  expect(workspace.quadratique("human", repository, "show", "--issue", "DEM-1")).toMatchObject({
    issue: { priority: 2 },
  });
});
