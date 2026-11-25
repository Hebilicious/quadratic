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
      "Write the guide",
      "--acceptance",
      "It is done.",
    );
    worktree = await workspace.worktree("agent-1", "guide-work");
    workspace.quadratique("agent", worktree, "claim", "--issue", "DEM-1");
    await writeFile(join(worktree, "guide.md"), "# The guide\n");
    await workspace.git(worktree, "add", "guide.md");
    await workspace.git(worktree, "commit", "--quiet", "--message", "Write the guide");
    workspace.quadratique(
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

test("the issue page links the pull request, the landed commit and the addresses in its text, and never shows the remote's token", async ({
  page,
  workspace,
}) => {
  const { repository } = workspace;
  const token = "ghp_MadeUpToken0123456789";
  const issue = page.getByRole("article");
  const work = issue.getByRole("complementary").getByRole("region", { name: "Work in git" });

  await test.step("an issue without work in git shows no work", async () => {
    await workspace.git(
      repository,
      "remote",
      "add",
      "origin",
      `https://made-up-user:${token}@github.com/Owner/demo.git`,
    );
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
      "Write the guide",
      "--description",
      "Follow https://example.com/style-guide. <b>Bold</b> stays text.",
      "--acceptance",
      "It passes https://example.com/checklist.",
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
      "--notes",
      "Compare with <https://example.com/old-guide>.",
    );
    await page.goto(workspace.url("/issues/DEM-1"));
    await expect(issue).toContainText("Write the guide");
    await expect(work).toHaveCount(0);
  });

  await test.step("addresses in the description, the acceptance and a note are links, and markup is shown as text", async () => {
    for (const address of ["https://example.com/checklist", "https://example.com/old-guide"]) {
      await expect(issue.getByRole("link", { name: address })).toHaveAttribute("href", address);
    }
    await expect(issue.getByRole("region", { name: "Notes" })).toContainText(
      "Compare with <https://example.com/old-guide>.",
    );
    await expect(
      issue.getByRole("link", { name: "https://example.com/style-guide" }),
    ).toHaveAttribute("href", "https://example.com/style-guide");
    await expect(issue).toContainText("<b>Bold</b> stays text.");
    await expect(issue.locator("b")).toHaveCount(0);
  });

  let landed = "";
  await test.step("an agent claims it on a branch, closes it with a pull request address in the evidence, and it lands with Refs", async () => {
    const worktree = await workspace.worktree("agent-1", "guide-work");
    workspace.quadratique("agent", worktree, "claim", "--issue", "DEM-1");
    await writeFile(join(worktree, "guide.md"), "# The guide\n");
    await workspace.git(worktree, "add", "guide.md");
    await workspace.git(worktree, "commit", "--quiet", "--message", "Write the guide");
    workspace.quadratique(
      "agent",
      worktree,
      "close",
      "--issue",
      "DEM-1",
      "--evidence",
      "PR https://github.com/Owner/demo/pull/7.",
    );
    await workspace.git(repository, "merge", "--squash", "guide-work");
    await workspace.git(
      repository,
      "commit",
      "--quiet",
      "--message",
      "Write the guide\n\nRefs: DEM-1",
    );
    landed = await workspace.gitOutput(repository, "rev-parse", "HEAD");
  });

  await test.step("the page shows the branch, the pull request link and the landed commit link", async () => {
    await page.goto(workspace.url("/issues/DEM-1"));
    await expect(issue).toContainText("Merged");
    await expect(work).toContainText("guide-work");
    await expect(work.getByRole("link", { name: "Open on GitHub" })).toHaveAttribute(
      "href",
      "https://github.com/Owner/demo/pull/guide-work",
    );
    await expect(work.getByRole("link", { name: landed.slice(0, 12) })).toHaveAttribute(
      "href",
      `https://github.com/Owner/demo/commit/${landed}`,
    );
    await expect(
      issue.getByRole("link", { name: "https://github.com/Owner/demo/pull/7" }),
    ).toHaveAttribute("href", "https://github.com/Owner/demo/pull/7");
  });

  await test.step("the remote's user name and token are nowhere on the page", async () => {
    const html = await page.content();
    expect(html).not.toContain(token);
    expect(html).not.toContain("made-up-user");
  });
});
