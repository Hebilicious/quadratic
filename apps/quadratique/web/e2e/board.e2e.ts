import type { Page } from "@playwright/test";

import { expect, test } from "./workspace.ts";

test.use({ viewport: { width: 1440, height: 900 } });

const item = (page: Page, status: string, issue: string) =>
  page.getByRole("list", { name: status }).getByRole("listitem").filter({ hasText: issue });

test("the human moves cards on the board by the keyboard and by dragging, and the list agrees", async ({
  page,
  workspace,
}) => {
  const { repository } = workspace;

  await test.step("seed a project with four issues", () => {
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
    for (const title of [
      "Serve the web app",
      "Show every claim",
      "Watch the refs",
      "Plan the now view",
    ]) {
      workspace.quadratique(
        "human",
        repository,
        "create",
        "--title",
        title,
        "--acceptance",
        "It is done.",
      );
    }
  });

  await test.step("open the board: one column per status of the workflow, in its order, all in a 1440-pixel window", async () => {
    await page.goto(workspace.url("/projects/DEM/board"));
    await expect(page.getByRole("status")).toHaveText("Live");
    await expect(page.getByRole("heading", { level: 2 })).toHaveText([
      /^Backlog/u,
      /^Todo/u,
      /^In progress/u,
      /^In review/u,
      /^Merged/u,
      /^Done/u,
      /^Canceled/u,
    ]);
    await expect(page.getByRole("region", { name: "Done" })).toBeInViewport({ ratio: 1 });
    await expect(page.getByRole("region", { name: "Canceled" })).toBeInViewport({ ratio: 1 });
  });

  await test.step("J puts the cursor on the first card, and Shift L moves it to the next column", async () => {
    await expect(item(page, "Todo", "DEM-1")).toBeVisible();
    await page.keyboard.press("j");
    await expect(item(page, "Todo", "DEM-1")).toHaveAttribute("aria-current", "true");
    await page.keyboard.press("Shift+L");
    await expect(item(page, "In progress", "DEM-1")).toBeVisible();
    await expect(item(page, "Todo", "DEM-1")).toHaveCount(0);
  });

  await test.step("a card dragged onto another column moves there", async () => {
    await item(page, "Todo", "DEM-2").dragTo(page.getByRole("region", { name: "In review" }));
    await expect(item(page, "In review", "DEM-2")).toBeVisible();
    await expect(item(page, "Todo", "DEM-2")).toHaveCount(0);
  });

  await test.step("the cards left in Todo keep their board order", async () => {
    await expect(page.getByRole("list", { name: "Todo" }).getByRole("listitem")).toHaveText([
      /DEM-3/u,
      /DEM-4/u,
    ]);
  });

  await test.step("Mod B switches to the list, which shows the same statuses and the same order", async () => {
    await page.keyboard.press("ControlOrMeta+b");
    await expect(page).toHaveURL(/\/projects\/DEM$/u);
    await expect(item(page, "In progress", "DEM-1")).toContainText("Serve the web app");
    await expect(item(page, "In review", "DEM-2")).toContainText("Show every claim");
    await expect(page.getByRole("list", { name: "Todo" }).getByRole("listitem")).toHaveText([
      /DEM-3/u,
      /DEM-4/u,
    ]);
  });

  expect(workspace.quadratique("human", repository, "show", "--issue", "DEM-1")).toMatchObject({
    issue: { status: "in-progress" },
  });
  expect(workspace.quadratique("human", repository, "show", "--issue", "DEM-2")).toMatchObject({
    issue: { status: "in-review" },
  });
});
