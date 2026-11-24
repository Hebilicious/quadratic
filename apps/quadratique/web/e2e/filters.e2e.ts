import type { Page } from "@playwright/test";

import { expect, test } from "./workspace.ts";

test.use({ viewport: { width: 1440, height: 900 } });

const rows = (page: Page) => page.getByRole("main").getByRole("listitem");

test("the human filters the list by label and readiness, sees it follow the agents live, and keeps the filters as a saved view", async ({
  page,
  workspace,
}) => {
  const { repository } = workspace;

  await test.step("seed a project: two issues labelled web, one labelled server", () => {
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
      "--labels",
      "web",
    );
    workspace.quadratique(
      "human",
      repository,
      "create",
      "--title",
      "Show every claim",
      "--acceptance",
      "It is done.",
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
      "--labels",
      "server",
    );
  });

  await test.step("filter by the label web and by ready: the address holds the filters, and the list shows the two ready web issues", async () => {
    await page.goto(workspace.url("/projects/DEM"));
    await expect(page.getByRole("status")).toHaveText("Live");
    await expect(rows(page)).toHaveText([/DEM-1/u, /DEM-2/u, /DEM-3/u]);
    await page.getByText("Label", { exact: true }).click();
    await page.getByRole("group", { name: "Label" }).getByLabel("web").check();
    await page.getByRole("combobox", { name: "Ready", exact: true }).selectOption("true");
    await expect(page).toHaveURL(/\/projects\/DEM\?label=web&ready=true$/u);
    await expect(rows(page)).toHaveText([/DEM-1/u, /DEM-2/u]);
  });

  await test.step("an agent claims DEM-2 from the command line: it is no longer ready and leaves the list live; released, it comes back", async () => {
    const worktree = await workspace.worktree("agent-2", "dem-2-claims");
    workspace.quadratique("agent", worktree, "claim", "--issue", "DEM-2");
    await expect(rows(page)).toHaveText([/DEM-1/u]);
    workspace.quadratique("agent", worktree, "release", "--issue", "DEM-2");
    await expect(rows(page)).toHaveText([/DEM-1/u, /DEM-2/u]);
    workspace.quadratique("agent", worktree, "claim", "--issue", "DEM-2");
    await expect(rows(page)).toHaveText([/DEM-1/u]);
  });

  await test.step("save the filters as the view Ready web: it is listed in the sidebar", async () => {
    await page.getByRole("button", { name: "Save view" }).click();
    await page.getByLabel("Name of the view").fill("Ready web");
    await page.keyboard.press("Enter");
    await expect(
      page.getByRole("navigation").getByRole("link", { name: "Ready web" }),
    ).toHaveAttribute("aria-current", "page");
  });

  await test.step("after a reload the filters and the saved view are still there", async () => {
    await page.reload();
    await expect(page.getByRole("status")).toHaveText("Live");
    await expect(page).toHaveURL(/\/projects\/DEM\?label=web&ready=true$/u);
    await expect(rows(page)).toHaveText([/DEM-1/u]);
    await expect(
      page.getByRole("navigation").getByRole("link", { name: "Ready web" }),
    ).toBeVisible();
  });

  await test.step("from the unfiltered list, the saved view in the sidebar opens the filtered list", async () => {
    await page.getByRole("navigation").getByRole("link", { name: "Demo" }).click();
    await expect(page).toHaveURL(/\/projects\/DEM$/u);
    await expect(rows(page)).toHaveText([/DEM-1/u, /DEM-3/u, /DEM-2/u]);
    await page.getByRole("navigation").getByRole("link", { name: "Ready web" }).click();
    await expect(page).toHaveURL(/\/projects\/DEM\?label=web&ready=true$/u);
    await expect(rows(page)).toHaveText([/DEM-1/u]);
  });

  await test.step("closing the Save view and Rename forms gives focus back to what opened them, and the delete confirmation takes focus", async () => {
    const saveView = page.getByRole("button", { name: "Save view" });
    await saveView.click();
    await page.keyboard.press("Escape");
    await expect(saveView).toBeFocused();
    const edit = page.getByRole("button", { name: "Rename or delete the view Ready web" });
    await edit.click();
    await page.keyboard.press("Escape");
    await expect(edit).toBeFocused();
    await edit.click();
    await page.getByRole("button", { name: "Delete…" }).click();
    const keep = page.getByRole("button", { name: "Keep it" });
    await expect(keep).toBeFocused();
    await keep.click();
    await expect(edit).toBeFocused();
  });

  await test.step("on the filters of Ready web, Save view starts from its name; when this browser's storage is full, the form stays open and says so", async () => {
    await page.getByRole("button", { name: "Save view" }).click();
    const name = page.getByLabel("Name of the view");
    await expect(name).toHaveValue("Ready web");
    const filled = await page.evaluate(() => {
      let size = 1 << 20;
      let count = 0;
      while (size >= 1) {
        try {
          localStorage.setItem(`fill-${String(count)}`, "x".repeat(size));
          count += 1;
        } catch {
          size = Math.floor(size / 2);
        }
      }
      return count;
    });
    expect(filled).toBeGreaterThan(0);
    await name.fill("Ready web, again");
    await page.keyboard.press("Enter");
    await expect(page.getByRole("alert")).toContainText("could not be saved");
    await expect(name).toHaveValue("Ready web, again");
    await page.evaluate(() => {
      for (const key of Object.keys(localStorage).filter((each) => each.startsWith("fill-"))) {
        localStorage.removeItem(key);
      }
    });
  });

  await test.step("a label that no issue carries any more, chosen in the address, stays listed, checked, and can be removed", async () => {
    await page.goto(workspace.url("/projects/DEM?label=gone"));
    await expect(page.getByRole("status")).toHaveText("Live");
    await page.locator("summary", { hasText: /^Label/u }).click();
    const gone = page.getByRole("group", { name: "Label" }).getByLabel("gone");
    await expect(gone).toBeChecked();
    await gone.click();
    await expect(page).toHaveURL(/\/projects\/DEM$/u);
  });

  await test.step("the controls stop at the catalog's caps: an eleventh label cannot be chosen, and the search field keeps 200 characters", async () => {
    const labels = Array.from({ length: 11 }, (_, index) => `cap${String(index)}`);
    workspace.quadratique(
      "human",
      repository,
      "create",
      "--title",
      "Carry many labels",
      "--acceptance",
      "It is done.",
      ...labels.flatMap((label) => ["--labels", label]),
    );
    await page.goto(workspace.url("/projects/DEM"));
    await expect(page.getByRole("status")).toHaveText("Live");
    await page.locator("summary", { hasText: /^Label/u }).click();
    const panel = page.getByRole("group", { name: "Label" });
    for (const label of labels.slice(0, 10)) {
      await panel.getByLabel(label, { exact: true }).check();
    }
    await expect(panel.getByLabel("cap9", { exact: true })).toBeChecked();
    await expect(panel.getByLabel("web", { exact: true })).toBeDisabled();
    await expect(panel).toContainText("At most 10 labels at once");
    const search = page.getByRole("searchbox", { name: "Search" });
    await search.fill("𝒜".repeat(201));
    await expect(search).toHaveValue("𝒜".repeat(200));
    await expect
      .poll(() => [...(new URL(page.url()).searchParams.get("text") ?? "")].length)
      .toBe(200);
  });
});
