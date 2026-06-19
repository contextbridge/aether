import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { Workspace } from "../../src/evals/Workspace.js";

describe("createWorkspace", () => {
  it("creates an empty workspace", async () => {
    const ws = await Workspace.empty();
    cleanups.push(ws.rootPath);

    expect(existsSync(ws.rootPath)).toBe(true);
    expect(ws.path).toBe(ws.rootPath);
    expect(ws.source).toBe("local");
  });

  it("writes inline files, including nested paths", async () => {
    const ws = await Workspace.fromFiles({
      "notes.txt": "alpha\n",
      "dir/deep.txt": "deep",
    });
    cleanups.push(ws.rootPath);

    expect(await readFile(join(ws.path, "notes.txt"), "utf8")).toBe("alpha\n");
    expect(await readFile(join(ws.path, "dir/deep.txt"), "utf8")).toBe("deep");
  });

  it("rejects inline file paths that escape the workspace root", async () => {
    await expect(Workspace.fromFiles({ "../evil.txt": "x" })).rejects.toThrow(
      /escapes the workspace root/,
    );
  });

  it("copies a directory relative to the base dir", async () => {
    const baseDir = track(await mkdtemp(join(tmpdir(), "ws-base-")));
    await writeFile(join(baseDir, "fixture.txt"), "from dir");

    const ws = await Workspace.fromDir(".", baseDir);
    cleanups.push(ws.rootPath);

    expect(await readFile(join(ws.path, "fixture.txt"), "utf8")).toBe(
      "from dir",
    );
  });

  it("clones and checks out a git workspace, honoring a subdir", async () => {
    const repo = track(await mkdtemp(join(tmpdir(), "ws-repo-")));
    const git = (...args: string[]) =>
      execFileSync("git", args, { cwd: repo, stdio: "pipe" });
    git("init", "--initial-branch", "main");
    git("config", "user.email", "eval@example.com");
    git("config", "user.name", "Eval");
    await writeFile(join(repo, "root.txt"), "root\n");
    await mkdir(join(repo, "pkg"), { recursive: true });
    await writeFile(join(repo, "pkg", "inner.txt"), "inner\n");
    git("add", ".");
    git("commit", "-m", "init");
    const startCommit = git("rev-parse", "HEAD").toString().trim();

    const ws = await Workspace.fromGitRepo({
      url: `file://${repo}`,
      startCommit,
      goldCommit: startCommit,
      subdir: "pkg",
    });
    cleanups.push(ws.rootPath);

    expect(ws.relativeCwd).toBe("pkg");
    expect(ws.path).toBe(join(ws.rootPath, "pkg"));
    expect(await readFile(join(ws.rootPath, "root.txt"), "utf8")).toBe(
      "root\n",
    );
    expect(await readFile(join(ws.path, "inner.txt"), "utf8")).toBe("inner\n");
    expect(ws.source).toEqual({
      git: { url: `file://${repo}`, startCommit, goldCommit: startCommit },
    });
  });
});

const cleanups: string[] = [];

afterEach(async () => {
  for (const path of cleanups.splice(0)) {
    await rm(path, { recursive: true, force: true });
  }
});

function track(path: string): string {
  cleanups.push(path);
  return path;
}
