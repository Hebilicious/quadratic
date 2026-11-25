import { createServer, type ServerResponse } from "node:http";

import { expect, onTestFinished, test } from "vitest";

import { openQuadratique } from "./quadratique.ts";

test("the live connection opens only once the server has answered the subscription, after it registered its listener", async () => {
  let opened = false;
  const answered: boolean[] = [];
  const streams: ServerResponse[] = [];
  const server = createServer((request, response) => {
    if (request.method !== "POST" || request.url !== "/web/events/issue.changed") {
      response.writeHead(404).end();
      return;
    }
    answered.push(opened);
    response.writeHead(200, { "content-type": "text/event-stream" });
    response.write(":\n\n");
    streams.push(response);
  });
  await new Promise<void>((resolve) => {
    server.listen(0, "127.0.0.1", resolve);
  });
  onTestFinished(async () => {
    for (const stream of streams) {
      stream.end();
    }
    await new Promise<void>((resolve) => {
      server.close(() => {
        resolve();
      });
    });
  });
  const address = server.address();
  if (address === null || typeof address === "string") {
    throw new Error("The test server has no port");
  }
  const { port } = address;
  const quadratique = await openQuadratique({
    origin: `http://127.0.0.1:${String(port)}`,
    access: { token: "the-token", unauthorized: () => undefined },
    modelContext: undefined,
    writable: () => true,
  });
  onTestFinished(async () => {
    await quadratique.dispose();
  });
  const abort = new AbortController();
  onTestFinished(() => {
    abort.abort();
  });

  await quadratique.changes(undefined, abort.signal);
  opened = true;

  expect(answered).toEqual([false]);
});

test("while the page shows a past moment, no changing call leaves the page, and reads still do", async () => {
  const asked: string[] = [];
  const server = createServer((request, response) => {
    asked.push(request.url ?? "");
    response.writeHead(404).end();
  });
  await new Promise<void>((resolve) => {
    server.listen(0, "127.0.0.1", resolve);
  });
  onTestFinished(async () => {
    await new Promise<void>((resolve) => {
      server.close(() => {
        resolve();
      });
    });
  });
  const address = server.address();
  if (address === null || typeof address === "string") {
    throw new Error("The test server has no port");
  }
  const quadratique = await openQuadratique({
    origin: `http://127.0.0.1:${String(address.port)}`,
    access: { token: "the-token", unauthorized: () => undefined },
    modelContext: undefined,
    writable: () => false,
  });
  onTestFinished(async () => {
    await quadratique.dispose();
  });
  const signal = new AbortController().signal;

  await expect(
    quadratique.create(
      { project: "DEM", title: "Written to the past", acceptance: "It is done." },
      signal,
    ),
  ).rejects.toThrow();
  await expect(
    quadratique.update({ issue: "DEM-1", revision: 1, patch: { priority: 1 } }, signal),
  ).rejects.toThrow();
  await expect(quadratique.projects(signal)).rejects.toThrow();

  expect(asked).toEqual(["/web/capabilities/project.list"]);
});

test("every request, the event stream included, carries the bearer, and a 401 tells the page", async () => {
  const bearers: (string | undefined)[] = [];
  const server = createServer((request, response) => {
    bearers.push(request.headers.authorization);
    response.writeHead(401, { "content-type": "application/json" });
    response.end(JSON.stringify({ kind: "unavailable", message: "Not paired." }));
  });
  await new Promise<void>((resolve) => {
    server.listen(0, "127.0.0.1", resolve);
  });
  onTestFinished(async () => {
    await new Promise<void>((resolve) => {
      server.close(() => {
        resolve();
      });
    });
  });
  const address = server.address();
  if (address === null || typeof address === "string") {
    throw new Error("The test server has no port");
  }
  let refusals = 0;
  const quadratique = await openQuadratique({
    origin: `http://127.0.0.1:${String(address.port)}`,
    access: {
      token: "the-token",
      unauthorized: () => {
        refusals += 1;
      },
    },
    modelContext: undefined,
    writable: () => true,
  });
  onTestFinished(async () => {
    await quadratique.dispose();
  });
  const signal = new AbortController().signal;

  await expect(quadratique.projects(signal)).rejects.toThrow();
  await expect(quadratique.changes(undefined, signal)).rejects.toThrow();

  expect(bearers).toEqual(["Bearer the-token", "Bearer the-token"]);
  expect(refusals).toBe(2);
});
