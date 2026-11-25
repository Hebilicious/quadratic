import { createServer } from "node:http";

import { expect, test } from "./workspace.ts";

/**
 * Another site's page, which frames the app and calls its web route. Its script attaches the frame's `load`
 * listener before it sets the frame's `src`, so no order of parsing and loading can hide the load from it.
 *
 * With `held`, the server sends the page up to and including the frame, and holds the rest until `release`: the test
 * releases it only once the frame has ended on the browser's error page, the order in which a listener attached
 * later in the page would miss the load.
 */
async function foreignPage(app: string, held: boolean) {
  const frame = `<!doctype html>
<title>Another site</title>
<p id="call">calling</p>
<p id="framed">framing</p>
<script>
  const frame = document.createElement("iframe");
  frame.id = "frame";
  frame.title = "framed app";
  frame.addEventListener("load", () => {
    document.getElementById("framed").textContent = "loaded";
  });
  frame.src = ${JSON.stringify(app)};
  document.body.append(frame);
</script>
`;
  const call = `<script>
  fetch(new URL("/web/capabilities/issue.create", ${JSON.stringify(app)}), {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ project: "DEM", title: "Planted by another site", acceptance: "It is done." }),
  }).then(
    (response) => { document.getElementById("call").textContent = "answered " + response.status; },
    () => { document.getElementById("call").textContent = "refused"; },
  );
</script>`;
  let release = (): void => undefined;
  const released = new Promise<void>((resolve) => {
    release = resolve;
  });
  const server = createServer((_request, response) => {
    response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    if (!held) {
      response.end(frame + call);
      return;
    }
    response.write(frame);
    void released.then(() => response.end(call));
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
    release,
    close: async () => {
      release();
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

  for (const held of [false, true]) {
    const foreign = await foreignPage(app, held);
    try {
      const delivery = held ? "held until its frame has ended" : "whole";
      await test.step(`the other site's call is refused, and its frame shows no app (page ${delivery})`, async () => {
        if (held) {
          const framedEnded = page.waitForEvent(
            "framenavigated",
            (frame) => frame !== page.mainFrame() && frame.url().startsWith("chrome-error:"),
          );
          await page.goto(foreign.url, { waitUntil: "commit" });
          await framedEnded;
          foreign.release();
        } else {
          await page.goto(foreign.url);
        }
        await expect(page.locator("#call")).toHaveText("refused");
        await expect(page.locator("#framed")).toHaveText("loaded");
        const framed = page.frames().find((frame) => frame !== page.mainFrame());
        expect(framed?.url()).toMatch(/^chrome-error:/u);
      });
    } finally {
      await foreign.close();
    }
  }

  expect(
    workspace.quadratique("human", repository, "issue", "list", "--project", "DEM"),
  ).toMatchObject({
    items: [],
  });
});
