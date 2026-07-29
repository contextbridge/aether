import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import {
  mkdir,
  mkdtemp,
  readFile,
  rm,
  unlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { createGitBundle, Workspace } from "../src/workspace.js";

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

  it("persists a workspace and leaves cleanup with the caller", async () => {
    const ws = await Workspace.fromFiles({ "notes.txt": "keep\n" });
    const retained = ws.persist();
    cleanups.push(retained.rootPath);

    await ws.cleanup();

    expect(retained.path).toBe(ws.path);
    expect(existsSync(retained.rootPath)).toBe(true);
    expect(await readFile(ws.join("notes.txt"), "utf8")).toBe("keep\n");
  });

  it("captures agent and reference git diffs with stats", async () => {
    const repo = track(await mkdtemp(join(tmpdir(), "ws-repo-")));
    const git = (...args: string[]) =>
      execFileSync("git", args, { cwd: repo, stdio: "pipe" });
    git("init", "--initial-branch", "main");
    git("config", "user.email", "eval@example.com");
    git("config", "user.name", "Eval");
    await writeFile(join(repo, "app.txt"), "before\n");
    git("add", ".");
    git("commit", "-m", "start");
    const startCommit = git("rev-parse", "HEAD").toString().trim();

    await writeFile(join(repo, "app.txt"), "gold\n");
    await writeFile(join(repo, "gold.txt"), "added\n");
    git("add", ".");
    git("commit", "-m", "gold");
    const goldCommit = git("rev-parse", "HEAD").toString().trim();

    const ws = await Workspace.fromGitRepo({
      url: `file://${repo}`,
      startCommit,
      goldCommit,
    });
    cleanups.push(ws.rootPath);
    await writeFile(ws.join("app.txt"), "agent\n");

    const { agentDiff, referenceDiff } = await ws.captureGitDiffs();

    expect(agentDiff?.diff).toContain("+agent");
    expect(agentDiff?.stats).toMatchObject({
      filesChanged: 1,
      linesAdded: 1,
      linesRemoved: 1,
    });
    expect(referenceDiff?.diff).toContain("+gold");
    expect(referenceDiff?.stats.filesChanged).toBe(2);
  });

  it("captures committed agent changes from the start commit", async () => {
    const ws = await createGitWorkspace({ "app.txt": "before\n" });
    const workspaceGit = (...args: string[]) =>
      execFileSync("git", args, { cwd: ws.rootPath, stdio: "pipe" });
    await writeFile(ws.join("app.txt"), "agent committed\n");
    workspaceGit("add", ".");
    workspaceGit(
      "-c",
      "user.email=agent@example.com",
      "-c",
      "user.name=Agent",
      "commit",
      "-m",
      "agent change",
    );

    const { agentDiff } = await ws.captureGitDiffs();

    expect(agentDiff?.diff).toContain("+agent committed");
    expect(agentDiff?.stats).toMatchObject({
      filesChanged: 1,
      linesAdded: 1,
      linesRemoved: 1,
    });
  });

  it("captures staged, unstaged, added, and deleted tracked changes", async () => {
    const ws = await createGitWorkspace({
      "staged.txt": "before staged\n",
      "unstaged.txt": "before unstaged\n",
      "deleted.txt": "before deleted\n",
    });
    const workspaceGit = (...args: string[]) =>
      execFileSync("git", args, { cwd: ws.rootPath, stdio: "pipe" });

    await writeFile(ws.join("staged.txt"), "after staged\n");
    await writeFile(ws.join("added.txt"), "added\n");
    workspaceGit("add", "staged.txt", "added.txt");
    await writeFile(ws.join("unstaged.txt"), "after unstaged\n");
    await unlink(ws.join("deleted.txt"));

    const { agentDiff } = await ws.captureGitDiffs();

    expect(agentDiff?.diff).toContain("staged.txt");
    expect(agentDiff?.diff).toContain("+after staged");
    expect(agentDiff?.diff).toContain("unstaged.txt");
    expect(agentDiff?.diff).toContain("+after unstaged");
    expect(agentDiff?.diff).toContain("added.txt");
    expect(agentDiff?.diff).toContain("+added");
    expect(agentDiff?.diff).toContain("deleted.txt");
    expect(agentDiff?.diff).toContain("-before deleted");
    expect(agentDiff?.stats).toEqual({
      filesChanged: 4,
      linesAdded: 3,
      linesRemoved: 3,
    });
  });

  it("captures an unchanged workspace as an empty agent diff", async () => {
    const ws = await createGitWorkspace({ "app.txt": "unchanged\n" });

    const { agentDiff } = await ws.captureGitDiffs();

    expect(agentDiff).toEqual({
      diff: "",
      stats: { filesChanged: 0, linesAdded: 0, linesRemoved: 0 },
    });
  });

  it("creates a bundle and instantiates a workspace from it, honoring subdir and diffs", async () => {
    const repo = track(await mkdtemp(join(tmpdir(), "ws-repo-")));
    const git = (...args: string[]) =>
      execFileSync("git", args, { cwd: repo, stdio: "pipe" });
    git("init", "--initial-branch", "main");
    git("config", "user.email", "eval@example.com");
    git("config", "user.name", "Eval");
    await writeFile(join(repo, "root.txt"), "root v1\n");
    await mkdir(join(repo, "pkg"), { recursive: true });
    await writeFile(join(repo, "pkg", "inner.txt"), "inner v1\n");
    git("add", ".");
    git("commit", "-m", "start");
    const startCommit = git("rev-parse", "HEAD").toString().trim();

    await writeFile(join(repo, "root.txt"), "root v2\n");
    git("add", ".");
    git("commit", "-m", "gold");
    const goldCommit = git("rev-parse", "HEAD").toString().trim();

    const bundleDir = track(await mkdtemp(join(tmpdir(), "ws-bundle-")));
    const bundlePath = join(bundleDir, "repo.bundle");
    await createGitBundle(
      { url: `file://${repo}`, startCommit, goldCommit },
      bundlePath,
    );

    const ws = await Workspace.fromGitBundle({
      bundlePath,
      startCommit,
      goldCommit,
      subdir: "pkg",
    });
    cleanups.push(ws.rootPath);

    expect(ws.relativeCwd).toBe("pkg");
    expect(ws.path).toBe(join(ws.rootPath, "pkg"));
    expect(await readFile(join(ws.rootPath, "root.txt"), "utf8")).toBe(
      "root v1\n",
    );
    expect(ws.source).toEqual({ bundle: { startCommit, goldCommit } });

    await writeFile(ws.join("inner.txt"), "inner edited\n");
    const { agentDiff, referenceDiff } = await ws.captureGitDiffs();
    expect(referenceDiff?.diff).toContain("root v2");
    expect(agentDiff?.diff).toContain("inner edited");
  });

  it("rejects a missing bundle file", async () => {
    await expect(
      Workspace.fromGitBundle({
        bundlePath: join(tmpdir(), "does-not-exist.bundle"),
        startCommit: "abc",
        goldCommit: "def",
      }),
    ).rejects.toThrow(/git bundle file does not exist/);
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

async function createGitWorkspace(
  files: Record<string, string>,
): Promise<Workspace> {
  const repo = track(await mkdtemp(join(tmpdir(), "ws-repo-")));
  const git = (...args: string[]) =>
    execFileSync("git", args, { cwd: repo, stdio: "pipe" });
  git("init", "--initial-branch", "main");
  git("config", "user.email", "eval@example.com");
  git("config", "user.name", "Eval");
  for (const [path, content] of Object.entries(files)) {
    await writeFile(join(repo, path), content);
  }
  git("add", ".");
  git("commit", "-m", "start");
  const startCommit = git("rev-parse", "HEAD").toString().trim();
  const workspace = await Workspace.fromGitRepo({
    url: `file://${repo}`,
    startCommit,
    goldCommit: startCommit,
  });
  cleanups.push(workspace.rootPath);
  return workspace;
}
