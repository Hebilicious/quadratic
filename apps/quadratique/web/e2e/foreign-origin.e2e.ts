import { createServer } from "node:http";

import { expect, test } from "./workspace.ts";

async function foreignPage(app: string) {
  const html = `<!doctype html>
<title>Another site</title>
<p id="call">calling</p>
<iframe id="frame" src="${app}" title="framed app"></iframe>
<p id="framed">framing</p>
<script>
  document.getElementById("frame").addEventListener("load", () => {
    document.getElementById("framed").textContent = "loaded";
  });
  fetch(new URL("/web/capabilities/issue.create", ${JSON.stringify(app)}), {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ project: "DEM", title: "Planted by another site", acceptance: "It is done." }),
  }).then(
    (response) => { document.getElementById("call").textContent = "answered " + response.status; },
    () => { document.getElementById("call").textContent = "refused"; },
  );
</script>`;
  const server = createServer((_request, response) => {
    response.writeHead(200, { "content-type": "text/html; charset=utf-8" }).end(html);
  });
  await new Promise<void>((resolve) => {
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  if (address === null || typeof address === "string") {
    throw new Error("The foreign site has no port");
  }
  return {
    url: `http://127.0.0.1:${String(address.port)}/`,
    close: async () => {
      await new Promise<void>((resolve) => {
        server.close(() => {
          resolve();
        });
      });
    },
  };
}

test("a page of another origin can neither write through the web route nor frame the app", async ({
  page,
  workspace,
}) => {
  const { repository } = workspace;
  workspace.quadratique("human", repository, "project", "create", "--key", "DEM", "--name", "Demo");
  const app = workspace.url();

  await test.step("the app's own document carries its Content-Security-Policy", async () => {
    const response = await page.goto(app);
    const policy = response?.headers()["content-security-policy"] ?? "";
    expect(policy).toContain("default-src 'self'");
    expect(policy).toContain("frame-ancestors 'none'");
  });

  const foreign = await foreignPage(app);
  try {
    await test.step("the other site's call is refused, and its frame shows no app", async () => {
      await page.goto(foreign.url);
      await expect(page.locator("#call")).toHaveText("refused");
      await expect(page.locator("#framed")).toHaveText("loaded");
      const framed = page.frames().find((frame) => frame !== page.mainFrame());
      expect(framed?.url()).toMatch(/^chrome-error:/u);
    });
  } finally {
    await foreign.close();
  }

  expect(
    workspace.quadratique("human", repository, "issue", "list", "--project", "DEM"),
  ).toMatchObject({
    items: [],
  });
});
