import { expect, test } from "./workspace.ts";

test("the human finishes an issue with evidence, and the history reads a finish by the new and the old command the same", async ({
  page,
  workspace,
}) => {
  const { repository } = workspace;
  const issue = page.getByRole("article");

  await test.step("seed a project with two issues; an agent finishes DEM-2 with the old command `close`", async () => {
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
    for (const title of ["Write the guide", "Publish the guide"]) {
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
    const worktree = await workspace.worktree("agent-2", "dem-2-publish");
    workspace.quadratique("agent", worktree, "claim", "--issue", "DEM-2");
    workspace.quadratique(
      "agent",
      worktree,
      "close",
      "--issue",
      "DEM-2",
      "--evidence",
      "published",
    );
  });

  await test.step("the command menu's address for DEM-1 opens its Finish with evidence form, with focus on the evidence", async () => {
    await page.goto(workspace.url("/issues/DEM-1#finish"));
    const form = issue.getByRole("region", { name: "Finish with evidence" });
    await expect(form.getByRole("textbox", { name: "Evidence" })).toBeFocused();
    await expect(issue.getByRole("region", { name: /Close/u })).toHaveCount(0);
  });

  await test.step("Finish the issue moves DEM-1 to Merged with the evidence, and its history says finished with evidence", async () => {
    const form = issue.getByRole("region", { name: "Finish with evidence" });
    await form.getByRole("textbox", { name: "Evidence" }).fill("Read it twice.");
    await form.getByRole("button", { name: "Finish the issue" }).click();
    await expect(issue.getByRole("combobox", { name: "Status" })).toHaveValue("merged");
    await expect(issue).toContainText("Read it twice.");
    await expect(issue.getByRole("region", { name: "Finish with evidence" })).toHaveCount(0);
    const changes = issue.getByRole("region", { name: "Changes" });
    await changes.getByRole("button", { name: "Show the changes" }).click();
    await expect(changes).toContainText(/tester\s*finished with evidence/u);
  });

  await test.step("DEM-2, finished by the old command, reads the same in its history, and is in review", async () => {
    await page.goto(workspace.url("/issues/DEM-2"));
    await expect(issue).toContainText("published");
    await expect(issue.getByRole("combobox", { name: "Status" })).toHaveValue("in-review");
    const changes = issue.getByRole("region", { name: "Changes" });
    await changes.getByRole("button", { name: "Show the changes" }).click();
    await expect(changes).toContainText(/claude-code@agent-2\s*finished with evidence/u);
  });
});
