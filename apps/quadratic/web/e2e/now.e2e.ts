import { writeFile } from "node:fs/promises";
import { join } from "node:path";

import { expect, test } from "./workspace.ts";

test("the now view shows, live and across projects, what every agent is doing and what waits on the human", async ({
  page,
  workspace,
}) => {
  const { repository } = workspace;
  let website = "";

  await test.step("seed two projects, each with an issue for agents", async () => {
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
    website = await workspace.repositoryAt(join(workspace.root, "website"));
    workspace.quadratic("human", website, "project", "create", "--key", "WEB", "--name", "Website");
    workspace.quadratic(
      "human",
      website,
      "create",
      "--title",
      "Publish the guide",
      "--acceptance",
      "It is done.",
    );
  });

  let loads = 0;
  page.on("load", () => {
    loads += 1;
  });

  await test.step("the Now link opens the now view, where no agent holds anything yet", async () => {
    await page.goto(workspace.url());
    await expect(page.getByRole("status")).toHaveText("Live");
    await page.getByRole("link", { name: /^Now/u }).click();
    await expect(page).toHaveURL(/\/now$/u);
    await expect(page.getByRole("region", { name: /What every agent is doing/u })).toContainText(
      "No agent holds an issue.",
    );
  });

  await test.step("an agent claims an issue of each project in its own worktree, and both appear with their branches", async () => {
    const guide = await workspace.worktree("agent-1", "dem-1-guide");
    workspace.quadratic("agent", guide, "claim", "--issue", "DEM-1");
    const publish = await workspace.worktree("agent-2", "publish-guide", website);
    workspace.quadratic("agent", publish, "claim", "--issue", "WEB-1");

    const first = page.getByRole("region", { name: "claude-code@agent-1" });
    await expect(first).toContainText("DEM-1");
    await expect(first).toContainText("dem-1-guide");
    await expect(first).toContainText("In progress");
    const second = page.getByRole("region", { name: "claude-code@agent-2" });
    await expect(second).toContainText("WEB-1");
    await expect(second).toContainText("publish-guide");
  });

  await test.step("the second agent finishes on its branch, and its issue waits for a review", async () => {
    const publish = join(workspace.root, "agent-2");
    await writeFile(join(publish, "guide.html"), "<h1>The guide</h1>\n");
    await workspace.git(publish, "add", "guide.html");
    await workspace.git(publish, "commit", "--quiet", "--message", "Publish the guide");
    workspace.quadratic("agent", publish, "close", "--issue", "WEB-1", "--evidence", "guide.html");
    const review = page.getByRole("list", { name: "Waiting for review or merge" });
    await expect(review.getByRole("listitem")).toContainText(["Publish the guide"]);
  });

  await test.step("an issue assigned to the human appears under what waits on the human", async () => {
    workspace.quadratic(
      "human",
      repository,
      "create",
      "--title",
      "Decide the release date",
      "--acceptance",
      "It is done.",
      "--assignee",
      '{"kind":"human"}',
    );
    const assigned = page.getByRole("list", { name: "Assigned to you" });
    await expect(assigned.getByRole("listitem")).toContainText(["Decide the release date"]);
    await expect(page.getByRole("link", { name: /Now/u })).toContainText("2");
  });

  await test.step("the command menu goes to what waits on the human", async () => {
    await page.getByRole("link", { name: "DEM Demo" }).click();
    await expect(page).toHaveURL(/\/projects\/DEM$/u);
    await page.keyboard.press("ControlOrMeta+k");
    await page.keyboard.type("go to what waits on you");
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/\/now#waiting$/u);
    await expect(page.getByRole("heading", { name: /Waits on you/u })).toBeFocused();
  });

  expect(loads).toBe(1);
});
