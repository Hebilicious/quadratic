import type { Page } from "@playwright/test";

import { expect, test, type Workspace } from "./workspace.ts";

const NOT_PAIRED = "To open Quadratique in this browser, run quadratique ui --open in a terminal.";

const notPaired = (page: Page) => page.getByText(NOT_PAIRED);
const live = (page: Page) => page.getByRole("status");
const issueTitle = (page: Page, title: string) =>
  page.getByRole("article").filter({ hasText: title });

function seed(workspace: Workspace): void {
  const { repository } = workspace;
  workspace.quadratique("human", repository, "project", "create", "--key", "DEM", "--name", "Demo");
  workspace.quadratique(
    "human",
    repository,
    "create",
    "--title",
    "Serve the web app",
    "--acceptance",
    "It is done.",
  );
}

test("a person runs quadratique ui once and is in: the code leaves the address, and a reload and a second tab need nothing", async ({
  page,
  context,
  workspace,
}) => {
  seed(workspace);

  await test.step("a browser that was never let in shows the one sentence", async () => {
    await page.goto(workspace.bookmark("/projects/DEM"));
    await expect(notPaired(page)).toBeVisible();
  });

  await test.step("the link of quadratique ui lets it in, and the code is gone from the address", async () => {
    await page.goto(workspace.url("/projects/DEM"));
    await expect(live(page)).toHaveText("Live");
    expect(new URL(page.url()).hash).toBe("");
    expect(new URL(page.url()).pathname).toBe("/projects/DEM");
    await expect(page.getByRole("listitem").filter({ hasText: "DEM-1" })).toBeVisible();
  });

  await test.step("a reload loads the board with nothing run", async () => {
    await page.reload();
    await expect(live(page)).toHaveText("Live");
  });

  await test.step("a second tab of a bookmark is in too", async () => {
    const other = await context.newPage();
    await other.goto(workspace.bookmark("/issues/DEM-1"));
    await expect(issueTitle(other, "Serve the web app")).toBeVisible();
    await other.close();
  });

  await test.step("a used link no longer lets another browser in", async () => {
    const link = workspace.url("/");
    const first = await context.browser()?.newContext();
    const second = await context.browser()?.newContext();
    if (first === undefined || second === undefined) {
      throw new Error("No browser for another context");
    }
    try {
      const one = await first.newPage();
      await one.goto(link);
      await expect(live(one)).toHaveText("Live");
      const two = await second.newPage();
      await two.goto(link);
      await expect(notPaired(two)).toBeVisible();
    } finally {
      await first.close();
      await second.close();
    }
  });
});

test("after the server restarts on its kept address, the open page and a bookmarked issue still work with nothing run", async ({
  page,
  context,
  workspace,
}) => {
  seed(workspace);
  await workspace.stop();
  await workspace.start("kept");
  const address = workspace.address;

  await test.step("open the board through quadratique ui", async () => {
    await page.goto(workspace.url("/projects/DEM"));
    await expect(live(page)).toHaveText("Live");
  });

  await test.step("the server restarts on the same address; the open page is live again", async () => {
    await workspace.stop();
    await expect(live(page)).toHaveText("Reconnecting");
    await workspace.start("kept");
    expect(workspace.address).toBe(address);
    await expect(live(page)).toHaveText("Live");
  });

  await test.step("a bookmark of the issue opens in a new tab", async () => {
    const other = await context.newPage();
    await other.goto(`http://${address}/issues/DEM-1`);
    await expect(issueTitle(other, "Serve the web app")).toBeVisible();
    await other.close();
  });
});

test("after server sign-out every tab shows the one sentence, and quadratique ui lets the browser in again", async ({
  page,
  context,
  workspace,
}) => {
  seed(workspace);
  await page.goto(workspace.url("/projects/DEM"));
  await expect(live(page)).toHaveText("Live");
  const other = await context.newPage();
  await other.goto(workspace.bookmark("/projects/DEM"));
  await expect(live(other)).toHaveText("Live");

  await test.step("the token is rotated; the next request of a tab is refused, and both tabs show the sentence", async () => {
    workspace.quadratique("human", workspace.repository, "server", "sign-out");
    await page.reload();
    await expect(notPaired(page)).toBeVisible();
    await expect(notPaired(other)).toBeVisible();
    expect(await page.evaluate(() => localStorage.getItem("quadratique.token"))).toBeNull();
  });

  await test.step("the link of quadratique ui lets the browser in again, in both tabs", async () => {
    await page.goto(workspace.url("/projects/DEM"));
    await expect(live(page)).toHaveText("Live");
    await expect(live(other)).toHaveText("Live");
  });
});

test("in strict mode a restart signs the browser out, and quadratique ui lets it in again", async ({
  page,
  workspace,
}) => {
  seed(workspace);
  workspace.quadratique("human", workspace.repository, "server", "strict", "on");
  await workspace.stop();
  await workspace.start();
  const port = Number(workspace.address.split(":").at(-1));
  await page.goto(workspace.url("/projects/DEM"));
  await expect(live(page)).toHaveText("Live");

  await test.step("the server restarts with a new token; the open page shows the sentence", async () => {
    await workspace.stop();
    await workspace.start(port);
    await expect(notPaired(page)).toBeVisible();
  });

  await test.step("the link of quadratique ui lets it in again", async () => {
    await page.goto(workspace.url("/projects/DEM"));
    await expect(live(page)).toHaveText("Live");
  });
});

test("a second link of quadratique ui, or a fake one, reaches an open tab without losing what the person typed", async ({
  page,
  workspace,
}) => {
  seed(workspace);
  const issue = page.getByRole("article");
  const description = issue.getByRole("textbox", { name: "Description" });
  await page.goto(workspace.url("/issues/DEM-1"));
  await expect(issue).toContainText("Serve the web app");
  await issue.getByRole("button", { name: "Edit description" }).click();
  await description.fill("Typed and not saved");

  await test.step("a second link of quadratique ui is exchanged while the page keeps the draft", async () => {
    const exchanged = page.waitForResponse(
      (answer) => new URL(answer.url()).pathname === "/session",
    );
    await page.goto(workspace.url("/issues/DEM-1"));
    expect((await exchanged).status()).toBe(200);
    await expect(page).toHaveURL(/\/issues\/DEM-1$/u);
    await expect(description).toHaveValue("Typed and not saved");
  });

  await test.step("a fake code is refused and changes nothing but the address", async () => {
    const refused = page.waitForResponse((answer) => new URL(answer.url()).pathname === "/session");
    await page.evaluate(() => {
      location.hash = "#pair=fake";
    });
    expect((await refused).status()).toBe(403);
    await expect(page).toHaveURL(/\/issues\/DEM-1$/u);
    await expect(description).toHaveValue("Typed and not saved");
    await expect(notPaired(page)).toHaveCount(0);
  });
});
