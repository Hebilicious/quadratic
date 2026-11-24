import { readFileSync } from "node:fs";

import { expect, test } from "./workspace.ts";

test("the human edits an issue, relates it, and loses nothing typed when the issue changed meanwhile", async ({
  page,
  workspace,
}) => {
  const { repository } = workspace;
  const issue = page.getByRole("article");

  await test.step("seed a project with two issues", () => {
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
    workspace.quadratic(
      "human",
      repository,
      "create",
      "--title",
      "Publish the guide",
      "--acceptance",
      "It is done.",
    );
  });

  await test.step("open the issue page of DEM-1", async () => {
    await page.goto(workspace.url("/issues/DEM-1"));
    await expect(issue).toContainText("Write the guide");
    await expect(issue).toContainText("Ready to start.");
  });

  await test.step("edit the description", async () => {
    await issue.getByRole("button", { name: "Edit description" }).click();
    await issue.getByRole("textbox", { name: "Description" }).fill("Cover the board and the keys.");
    await issue.getByRole("button", { name: "Save", exact: true }).click();
    await expect(issue.getByRole("textbox", { name: "Description" })).toHaveCount(0);
    await expect(issue).toContainText("Cover the board and the keys.");
  });

  await test.step("a note typed while an agent added one is refused as stale, kept, and added after it", async () => {
    const notes = issue.getByRole("region", { name: "Notes" });
    await notes.getByRole("button", { name: "Add a note" }).click();
    const field = notes.getByRole("textbox", { name: "New note" });
    await field.fill("Check the keys on a phone too.");
    workspace.quadratic(
      "agent",
      repository,
      "issue",
      "update",
      "--issue",
      "DEM-1",
      "--revision",
      workspace.revision("DEM-1"),
      "--notes",
      "Started on the board chapter.",
    );
    await expect(notes).toContainText("Started on the board chapter.");
    await notes.getByRole("button", { name: "Add the note" }).click();
    await expect(notes.getByRole("alert")).toContainText("changed since it was read");
    await expect(field).toHaveValue("Check the keys on a phone too.");
    await notes.getByRole("button", { name: "Add again" }).click();
    await expect(field).toHaveCount(0);
    const written = notes.getByRole("listitem");
    await expect(written).toHaveCount(2);
    await expect(written.nth(0)).toContainText("claude-code");
    await expect(written.nth(0)).toContainText("Started on the board chapter.");
    await expect(written.nth(1)).toContainText("tester");
    await expect(written.nth(1)).toContainText("Check the keys on a phone too.");
  });

  await test.step("DEM-1 blocks DEM-2, and DEM-2 shows that it is not ready, and why", async () => {
    await issue.getByRole("combobox", { name: "Related issue" }).selectOption("DEM-2");
    await issue.getByRole("button", { name: "Add", exact: true }).click();
    const relations = issue.getByRole("region", { name: "Relations" });
    await expect(relations).toContainText("Blocks");
    await relations.getByRole("link", { name: "DEM-2" }).click();
    await expect(page).toHaveURL(/\/issues\/DEM-2$/u);
    const why = issue.getByRole("region", { name: "Why it is not ready" });
    await expect(why).toContainText("It waits on an unresolved blocker");
    await expect(why.getByRole("link", { name: "DEM-1" })).toBeVisible();
  });

  await test.step("removing the relation makes DEM-2 ready again", async () => {
    await issue.getByRole("button", { name: "Remove: Blocked by DEM-1" }).click();
    await expect(issue).toContainText("Ready to start.");
    await expect(issue.getByRole("region", { name: "Why it is not ready" })).toHaveCount(0);
  });

  await test.step("a description typed while DEM-2 changed from the command line is refused as stale, and kept", async () => {
    await issue.getByRole("button", { name: "Edit description" }).click();
    const field = issue.getByRole("textbox", { name: "Description" });
    await field.fill("Publish it on the website.");
    workspace.quadratic(
      "human",
      repository,
      "issue",
      "update",
      "--issue",
      "DEM-2",
      "--revision",
      workspace.revision("DEM-2"),
      "--title",
      "Publish the guide on the website",
    );
    await expect(issue).toContainText("Publish the guide on the website");
    await issue.getByRole("button", { name: "Save", exact: true }).click();
    await expect(issue.getByRole("alert")).toContainText("changed since it was read");
    await expect(field).toHaveValue("Publish it on the website.");
    await issue.getByRole("button", { name: "Save again" }).click();
    await expect(field).toHaveCount(0);
    await expect(issue).toContainText("Publish it on the website.");
  });

  expect(workspace.quadratic("human", repository, "show", "--issue", "DEM-2")).toMatchObject({
    issue: {
      title: "Publish the guide on the website",
      description: "Publish it on the website.",
    },
  });
});

const SITE_RUN = readFileSync(
  new URL("../../server/tests/fixtures/findings/site.json", import.meta.url),
  "utf8",
);

test("an issue from a checker's finding shows its source and its links", async ({
  page,
  workspace,
}) => {
  const { repository } = workspace;
  const issue = page.getByRole("article");

  await test.step("import a run of a linter", () => {
    workspace.quadratic(
      "human",
      repository,
      "project",
      "create",
      "--key",
      "SITE",
      "--name",
      "Site",
    );
    workspace.quadratic(
      "human",
      repository,
      "findings",
      "import",
      "--tool",
      "linter",
      "--findings",
      SITE_RUN,
    );
  });

  await test.step("the issue of the first finding names the checker, the finding's key, its fact and its files", async () => {
    await page.goto(workspace.url("/issues/SITE-1"));
    const source = issue.getByRole("region", { name: "Source" });
    await expect(source).toContainText("linter");
    await expect(source).toContainText("L311 f:c7de2a79fd61899b");
    const links = source.getByRole("list", { name: "Links" }).getByRole("listitem");
    await expect(links).toHaveCount(3);
    await expect(links.nth(0)).toContainText("guide.install.nodeVersion");
    await expect(links.nth(1)).toContainText("guide/install.md");
    await expect(links.nth(2)).toContainText("notes/releases.md");
  });
});
