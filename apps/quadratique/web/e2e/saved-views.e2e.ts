import { createServer } from "node:net";

import type { Page } from "@playwright/test";

import { expect, test } from "./workspace.ts";

test.use({ viewport: { width: 1440, height: 900 } });

const LEGACY_KEY = "quadratique.saved-views";

const viewNames = (page: Page) =>
  page
    .getByRole("navigation")
    .getByRole("link")
    .filter({ has: page.locator(".nav-key", { hasText: "≡" }) })
    .locator(".nav-name");

/**
 * Holds `port` on the loopback interface so that the system cannot give it to the next server. When another process
 * already holds it, that process keeps it from the next server just as well.
 */
async function hold(port: number): Promise<{ readonly release: () => Promise<void> }> {
  const holder = createServer();
  const held = await new Promise<boolean>((resolve) => {
    holder.once("error", () => {
      resolve(false);
    });
    holder.listen(port, "127.0.0.1", () => {
      resolve(true);
    });
  });
  return {
    release: async () => {
      if (held) {
        await new Promise<void>((resolve) => {
          holder.close(() => {
            resolve();
          });
        });
      }
    },
  };
}

const portOf = (address: string): number => Number(address.split(":").at(-1));

test("saved views live in the server: one page's view shows in another, survives a restart on another port, and a browser's old views are imported once", async ({
  context,
  page,
  workspace,
}) => {
  const { repository } = workspace;

  await test.step("seed a project, open its list", async () => {
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
    await page.goto(workspace.url("/projects/DEM"));
    await expect(page.getByRole("status")).toHaveText("Live");
    await expect(viewNames(page)).toHaveCount(0);
  });

  await test.step("a view this browser kept before the server kept views is imported at the next load, and the browser forgets it", async () => {
    await page.evaluate((key) => {
      localStorage.setItem(
        key,
        JSON.stringify({
          version: 1,
          views: [{ id: "v1", name: "From this browser", project: "DEM", filter: "claim=mine" }],
        }),
      );
    }, LEGACY_KEY);
    await page.reload();
    await expect(viewNames(page)).toHaveText(["From this browser"]);
    await expect
      .poll(() => page.evaluate((key) => localStorage.getItem(key), LEGACY_KEY))
      .toBeNull();
    await page.reload();
    await expect(page.getByRole("status")).toHaveText("Live");
    await expect(viewNames(page)).toHaveText(["From this browser"]);
  });

  const other = await context.newPage();

  await test.step("a view saved in one page shows in another page of the same server", async () => {
    await other.goto(workspace.url("/projects/DEM"));
    await expect(other.getByRole("status")).toHaveText("Live");
    await expect(viewNames(other)).toHaveText(["From this browser"]);

    await page.goto(workspace.url("/projects/DEM?ready=true"));
    await expect(page.getByRole("status")).toHaveText("Live");
    await page.getByRole("button", { name: "Save view" }).click();
    await page.getByLabel("Name of the view").fill("Ready");
    await page.keyboard.press("Enter");
    await expect(page.getByRole("navigation").getByRole("link", { name: "Ready" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    await expect(viewNames(other)).toHaveText(["From this browser", "Ready"]);
  });

  await test.step("a rename in the other page shows in the first", async () => {
    await other.getByRole("button", { name: "Rename or delete the view Ready" }).click();
    await other.getByLabel("New name of the view Ready").fill("Ready now");
    await other.keyboard.press("Enter");
    await expect(viewNames(page)).toHaveText(["From this browser", "Ready now"]);
  });

  await test.step("after a restart of the server on another port, a page of the new address shows the same views", async () => {
    const before = portOf(workspace.address);
    await other.close();
    await workspace.stop();
    const old = await hold(before);
    try {
      await workspace.start();
    } finally {
      await old.release();
    }
    const port = portOf(workspace.address);
    expect(port).not.toBe(before);
    await page.goto(workspace.url("/projects/DEM"));
    await expect(page).toHaveURL(new RegExp(`^http://[^/]+:${String(port)}/`, "u"));
    await expect(page.getByRole("status")).toHaveText("Live");
    await expect(viewNames(page)).toHaveText(["From this browser", "Ready now"]);
    expect(await page.evaluate((key) => localStorage.getItem(key), LEGACY_KEY)).toBeNull();
  });
});
