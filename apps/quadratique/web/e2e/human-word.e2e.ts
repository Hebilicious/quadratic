import { expect, test } from "./workspace.ts";

const WORDS = "Yes, MIT. Close it.";

test("an agent moves a decision on the human's words, and the issue page shows the words and the confidence", async ({
  page,
  workspace,
}) => {
  const { repository } = workspace;
  const issue = page.getByRole("article");

  await test.step("seed a decision assigned to the human, then the agent moves it with the human's words at high confidence", () => {
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
      "Choose the license",
      "--acceptance",
      "The human chose a license.",
      "--assignee",
      '{"kind":"human"}',
    );
    workspace.quadratique(
      "agent",
      repository,
      "issue",
      "update",
      "--issue",
      "DEM-1",
      "--revision",
      workspace.revision("DEM-1"),
      "--status",
      "done",
      "--humanWords",
      WORDS,
      "--confidence",
      "high",
    );
  });

  await test.step("the note quotes the words and states the confidence, and the change gives them as its reason", async () => {
    await page.goto(workspace.url("/issues/DEM-1"));
    await expect(issue).toContainText("Choose the license");
    const notes = issue.getByRole("region", { name: "Notes" });
    await expect(notes).toContainText(
      `acted on the human's words, with high confidence: "${WORDS}"`,
    );
    const changes = issue.getByRole("region", { name: "Changes" });
    await changes.getByRole("button", { name: "Show the changes" }).click();
    await expect(changes).toContainText(`The human's words, high confidence: "${WORDS}"`);
  });
});
