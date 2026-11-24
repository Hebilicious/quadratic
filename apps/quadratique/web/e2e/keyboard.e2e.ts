import type { Page } from "@playwright/test";

import { expect, test } from "./workspace.ts";

const row = (page: Page, status: string, issue: string) =>
  page.getByRole("list", { name: status }).getByRole("listitem").filter({ hasText: issue });

test("the human triages with the keyboard alone: create, move, peek, set the priority and the status", async ({
  page,
  workspace,
}) => {
  const { repository } = workspace;
  const peek = page.getByRole("complementary", { name: "Peek" });

  await test.step("seed a project with two issues, and open the list", async () => {
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
    await page.goto(workspace.url());
    await expect(page.getByRole("status")).toHaveText("Live");
    await expect(row(page, "Todo", "DEM-2")).toBeVisible();
  });

  await test.step("C opens the form; a title alone does not create the issue; with the acceptance criteria, Ctrl+Enter creates it under the cursor", async () => {
    await page.keyboard.press("c");
    const dialog = page.getByRole("dialog", { name: "New issue" });
    await expect(dialog).toBeVisible();
    await page.keyboard.type("Triage from the keyboard");
    await page.keyboard.press("Enter");
    await expect(dialog.getByRole("button", { name: "Create issue" })).toBeDisabled();
    await dialog
      .getByRole("textbox", { name: "Acceptance criteria" })
      .fill("The issue is on the board.");
    await page.keyboard.press("Control+Enter");
    await expect(dialog).toHaveCount(0);
    await expect(row(page, "Todo", "DEM-3")).toContainText("Triage from the keyboard");
    await expect(row(page, "Todo", "DEM-3")).toHaveAttribute("aria-current", "true");
  });

  await test.step("Space peeks at the issue beside the list", async () => {
    await page.keyboard.press("Space");
    await expect(peek).toContainText("Triage from the keyboard");
    await expect(peek.getByRole("combobox", { name: "Priority" })).toHaveValue("0");
  });

  await test.step("P and the menu set the priority to High", async () => {
    await page.keyboard.press("p");
    await expect(page.getByRole("dialog", { name: "Command menu" })).toBeVisible();
    await page.keyboard.type("high");
    await page.keyboard.press("Enter");
    await expect(peek.getByRole("combobox", { name: "Priority" })).toHaveValue("2");
  });

  await test.step("S and the menu move it to In progress, and the list shows it in that group", async () => {
    await page.keyboard.press("s");
    await page.keyboard.type("in progress");
    await page.keyboard.press("Enter");
    await expect(row(page, "In progress", "DEM-3")).toContainText("Triage from the keyboard");
    await expect(peek.getByRole("combobox", { name: "Status" })).toHaveValue("in-progress");
  });

  await test.step("Mod K opens the command menu, which sets the priority to Urgent", async () => {
    await page.keyboard.press("ControlOrMeta+k");
    await expect(page.getByRole("dialog", { name: "Command menu" })).toBeVisible();
    await page.keyboard.type("priority urgent");
    await page.keyboard.press("Enter");
    await expect(peek.getByRole("combobox", { name: "Priority" })).toHaveValue("1");
  });

  await test.step("a letter typed in a select of the peek fires no shortcut", async () => {
    await peek.getByRole("combobox", { name: "Status" }).focus();
    await page.keyboard.press("k");
    await expect(row(page, "In progress", "DEM-3")).toHaveAttribute("aria-current", "true");
    await row(page, "In progress", "DEM-3").click();
    await expect(row(page, "In progress", "DEM-3")).toBeFocused();
  });

  await test.step("the shortcuts keep Tab inside, and give focus back to the issue when they close", async () => {
    await page.keyboard.press("?");
    const help = page.getByRole("dialog", { name: "Keyboard shortcuts" });
    await expect(help).toBeVisible();
    for (const _ of [1, 2, 3]) {
      await page.keyboard.press("Tab");
      await expect(help.locator(":focus")).toHaveCount(1);
    }
    await page.keyboard.press("Escape");
    await expect(help).toHaveCount(0);
    await expect(row(page, "In progress", "DEM-3")).toBeFocused();
  });

  await test.step("K moves the cursor up, and the peek follows it", async () => {
    await page.keyboard.press("k");
    await expect(row(page, "Todo", "DEM-2")).toHaveAttribute("aria-current", "true");
    await expect(peek).toContainText("Show every claim");
  });

  await test.step("a description typed in the peek is kept while the cursor moves away and back", async () => {
    await peek.getByRole("button", { name: "Edit description" }).click();
    await peek.getByRole("textbox", { name: "Description" }).fill("Kept while the cursor moves");
    await row(page, "In progress", "DEM-3").click();
    await expect(peek).toContainText("Triage from the keyboard");
    await row(page, "Todo", "DEM-2").click();
    await expect(peek.getByRole("textbox", { name: "Description" })).toHaveValue(
      "Kept while the cursor moves",
    );
  });

  await test.step("the issue page that the peek opens still holds the description typed", async () => {
    await peek.getByRole("link", { name: /Open DEM-2/u }).click();
    await expect(page).toHaveURL(/\/issues\/DEM-2$/u);
    await expect(
      page.getByRole("article").getByRole("textbox", { name: "Description" }),
    ).toHaveValue("Kept while the cursor moves");
  });

  expect(workspace.quadratique("human", repository, "show", "--issue", "DEM-3")).toMatchObject({
    issue: { priority: 1, status: "in-progress" },
  });
});
