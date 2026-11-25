import { execFile, spawn, spawnSync, type ChildProcess } from "node:child_process";
import { watch } from "node:fs";
import { mkdir, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, URL } from "node:url";

import { test as base } from "@playwright/test";

import { HANG_GUARD_MS } from "./hang-guard.ts";

const BINARY = fileURLToPath(
  new URL(
    `../../target/debug/quadratique${process.platform === "win32" ? ".exe" : ""}`,
    import.meta.url,
  ),
);
const BUILT_APP = fileURLToPath(new URL("../dist", import.meta.url));

export type Who = "human" | "agent";

interface Owned {
  readonly child: ChildProcess;
  readonly exited: Promise<void>;
}

const gitEnvironment = (root: string) => ({
  PATH: process.env["PATH"] ?? "",
  HOME: root,
  GIT_CONFIG_NOSYSTEM: "1",
  GIT_CONFIG_GLOBAL: "/dev/null",
});

export class Workspace {
  readonly home: string;
  readonly repository: string;
  private server: Owned | undefined;
  address = "";

  constructor(readonly root: string) {
    this.home = join(root, "home");
    this.repository = join(root, "demo");
  }

  async setUp(): Promise<void> {
    // The server refuses a home that a group or other accounts may use.
    await mkdir(this.home, { mode: 0o700 });
    await this.repositoryAt(this.repository);
    await this.start();
  }

  async repositoryAt(path: string): Promise<string> {
    await mkdir(path);
    await this.git(path, "init", "--quiet", "--initial-branch=main");
    await this.git(path, "commit", "--quiet", "--allow-empty", "--message", "init");
    return path;
  }

  /** Starts `quadratique serve` on `port`, 0 letting the system choose; `"kept"` starts it as a person does, on the
   * port the home kept. */
  async start(port: number | "kept" = 0): Promise<string> {
    if (this.server !== undefined) {
      throw new Error("The workspace's server already runs");
    }
    const args = port === "kept" ? ["serve"] : ["serve", "--port", String(port)];
    const child = spawn(BINARY, args, {
      cwd: this.root,
      env: this.environment("human"),
      stdio: ["ignore", "ignore", "pipe"],
    });
    const exited = new Promise<void>((resolve) => {
      child.once("exit", () => {
        resolve();
      });
    });
    this.server = { child, exited };
    let log = "";
    child.stderr?.on("data", (chunk: Buffer) => {
      log += chunk.toString();
    });
    try {
      this.address = await this.published(child, exited);
      return this.address;
    } catch (error) {
      throw new Error(`quadratique serve did not start: ${String(error)}\n${log}`, {
        cause: error,
      });
    }
  }

  async stop(): Promise<void> {
    const owned = this.server;
    if (owned === undefined) {
      return;
    }
    this.server = undefined;
    if (owned.child.exitCode !== null || owned.child.signalCode !== null) {
      return;
    }
    owned.child.kill("SIGTERM");
    const ended = await Promise.race([
      owned.exited.then(() => true),
      new Promise<false>((resolve) => setTimeout(() => resolve(false), HANG_GUARD_MS)),
    ]);
    if (!ended) {
      owned.child.kill("SIGKILL");
      await owned.exited;
      throw new Error(`quadratique serve (pid ${String(owned.child.pid)}) ignored SIGTERM`);
    }
  }

  async worktree(directory: string, branch: string, repository = this.repository): Promise<string> {
    const path = join(this.root, directory);
    await this.git(repository, "worktree", "add", "--quiet", "-b", branch, path);
    return path;
  }

  revision(issue: string): string {
    const shown = this.quadratique("human", this.repository, "show", "--issue", issue);
    const answered =
      typeof shown === "object" && shown !== null && "issue" in shown ? shown.issue : undefined;
    if (typeof answered !== "object" || answered === null || !("revision" in answered)) {
      throw new Error(`quadratique show gave no revision for ${issue}`);
    }
    return String(answered.revision);
  }

  /** The address of `path` with a one-time code from `quadratique ui`, as a person's first visit opens it. */
  url(path = "/"): string {
    const answer = this.ui();
    const code = new URL(answer.link).hash;
    if (!code.startsWith("#pair=")) {
      throw new Error("quadratique ui gave a link without a code");
    }
    const url = new URL(path, answer.url);
    url.hash = code;
    return url.href;
  }

  /** The address of `path` with no code, as a bookmark keeps it. */
  bookmark(path = "/"): string {
    return new URL(path, this.ui().url).href;
  }

  ui(): { readonly url: string; readonly link: string } {
    const answer = this.quadratique("human", this.repository, "ui");
    if (
      typeof answer !== "object" ||
      answer === null ||
      !("url" in answer) ||
      !("link" in answer) ||
      typeof answer.url !== "string" ||
      typeof answer.link !== "string"
    ) {
      throw new Error("quadratique ui gave no url and link");
    }
    return { url: answer.url, link: answer.link };
  }

  quadratique(who: Who, directory: string, ...args: string[]): unknown {
    const result = spawnSync(BINARY, args, {
      cwd: directory,
      encoding: "utf8",
      env: this.environment(who),
      timeout: HANG_GUARD_MS,
    });
    if (result.status !== 0) {
      throw new Error(
        `quadratique ${args.join(" ")} exited ${String(result.status)}: ${result.stderr}`,
      );
    }
    // BOUNDARY: the command line writes JSON; each caller reads only what it checks.
    return JSON.parse(result.stdout);
  }

  async git(directory: string, ...args: string[]): Promise<void> {
    await new Promise<void>((resolve, reject) => {
      execFile(
        "git",
        ["-c", "user.name=Test", "-c", "user.email=test@example.invalid", ...args],
        { cwd: directory, env: gitEnvironment(this.root), timeout: HANG_GUARD_MS },
        (error) => {
          if (error === null) {
            resolve();
          } else {
            reject(error);
          }
        },
      );
    });
  }

  async gitOutput(directory: string, ...args: string[]): Promise<string> {
    return await new Promise<string>((resolve, reject) => {
      execFile(
        "git",
        args,
        { cwd: directory, env: gitEnvironment(this.root), timeout: HANG_GUARD_MS },
        (error, stdout) => {
          if (error === null) {
            resolve(stdout.trim());
          } else {
            reject(error);
          }
        },
      );
    });
  }

  private environment(who: Who): NodeJS.ProcessEnv {
    return {
      ...gitEnvironment(this.root),
      QUADRATIQUE_HOME: this.home,
      QUADRATIQUE_WEB: BUILT_APP,
      USER: "tester",
      ...(who === "agent" ? { CLAUDECODE: "1" } : {}),
    };
  }

  private async published(child: ChildProcess, exited: Promise<void>): Promise<string> {
    const runtime = join(this.home, "server.json");
    const read = async (): Promise<string | undefined> => {
      const text = await readFile(runtime, "utf8").catch((error: unknown) => {
        if (error instanceof Error && "code" in error && error.code === "ENOENT") {
          return undefined;
        }
        throw error;
      });
      if (text === undefined) {
        return undefined;
      }
      // BOUNDARY: the runtime file that the server writes; only its pid and address are read.
      const written: unknown = JSON.parse(text);
      if (
        typeof written === "object" &&
        written !== null &&
        "pid" in written &&
        "address" in written &&
        written.pid === child.pid
      ) {
        return String(written.address);
      }
      return undefined;
    };
    const watcher = watch(this.home);
    try {
      return await new Promise<string>((resolve, reject) => {
        const timer = setTimeout(() => {
          reject(new Error("no runtime file within the hang guard"));
        }, HANG_GUARD_MS);
        const check = () => {
          void read().then(
            (address) => {
              if (address !== undefined) {
                clearTimeout(timer);
                resolve(address);
              }
            },
            (error: unknown) => {
              clearTimeout(timer);
              reject(error);
            },
          );
        };
        watcher.on("change", check);
        void exited.then(() => {
          clearTimeout(timer);
          reject(new Error(`it exited with ${String(child.exitCode ?? child.signalCode)}`));
        });
        check();
      });
    } finally {
      watcher.close();
    }
  }

  async dispose(): Promise<void> {
    const failures: unknown[] = [];
    try {
      await this.stop();
    } catch (error) {
      failures.push(error);
    }
    try {
      await rm(this.root, { recursive: true, force: true, maxRetries: 3 });
    } catch (error) {
      failures.push(error);
    }
    if (failures.length > 0) {
      throw new AggregateError(failures, "The workspace was not cleaned up");
    }
  }
}

export const test = base.extend<{ workspace: Workspace }>({
  // oxlint-disable-next-line no-empty-pattern -- Playwright reads a fixture's dependencies from this object pattern, so it must be one, even empty.
  workspace: async ({}, use) => {
    const workspace = new Workspace(await mkdtemp(join(tmpdir(), "quadratique-journey-")));
    try {
      await workspace.setUp();
      await use(workspace);
    } finally {
      await workspace.dispose();
    }
  },
});

export { expect } from "@playwright/test";
