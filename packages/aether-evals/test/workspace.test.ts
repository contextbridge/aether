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
import { dirname, join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import {
  createGitBundle,
  type GitRepoSpec,
  Workspace,
} from "../src/workspace.js";

describe("createWorkspace", () => {
  it("creates an empty workspace without git diffs", async () => {
    const ws = await Workspace.empty();
    cleanups.push(ws.rootPath);

    expect(existsSync(ws.rootPath)).toBe(true);
    expect(ws.path).toBe(ws.rootPath);
    expect(ws.referenceDiff).toBeUndefined();
    expect(await ws.captureAgentDiff()).toBeUndefined();
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
    const ws = await gitWorkspace(
      { "root.txt": "root\n", "pkg/inner.txt": "inner\n" },
      {},
      { subdir: "pkg" },
    );

    expect(ws.relativeCwd).toBe("pkg");
    expect(ws.path).toBe(join(ws.rootPath, "pkg"));
    expect(await readFile(join(ws.rootPath, "root.txt"), "utf8")).toBe(
      "root\n",
    );
    expect(await readFile(join(ws.path, "inner.txt"), "utf8")).toBe("inner\n");
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
    const ws = await gitWorkspace(
      { "app.txt": "before\n" },
      { "app.txt": "gold\n", "gold.txt": "added\n" },
    );
    await writeFile(ws.join("app.txt"), "agent\n");

    const agentDiff = await ws.captureAgentDiff();

    expect(agentDiff?.diff).toContain("+agent");
    expect(agentDiff?.stats).toMatchObject({
      filesChanged: 1,
      linesAdded: 1,
      linesRemoved: 1,
    });
    expect(ws.referenceDiff?.diff).toContain("+gold");
    expect(ws.referenceDiff?.stats.filesChanged).toBe(2);
  });

  it("captures committed agent changes from the start commit", async () => {
    const ws = await gitWorkspace({ "app.txt": "before\n" });
    await writeFile(ws.join("app.txt"), "agent committed\n");
    git(ws.rootPath, "add", ".");
    git(
      ws.rootPath,
      "-c",
      "user.email=agent@example.com",
      "-c",
      "user.name=Agent",
      "commit",
      "-m",
      "agent change",
    );

    const agentDiff = await ws.captureAgentDiff();

    expect(agentDiff?.diff).toContain("+agent committed");
    expect(agentDiff?.stats).toMatchObject({
      filesChanged: 1,
      linesAdded: 1,
      linesRemoved: 1,
    });
  });

  it("captures staged, unstaged, added, deleted, and untracked changes", async () => {
    const ws = await gitWorkspace({
      "staged.txt": "before staged\n",
      "unstaged.txt": "before unstaged\n",
      "deleted.txt": "before deleted\n",
    });

    await writeFile(ws.join("staged.txt"), "after staged\n");
    await writeFile(ws.join("added.txt"), "added\n");
    git(ws.rootPath, "add", "staged.txt", "added.txt");
    await writeFile(ws.join("unstaged.txt"), "after unstaged\n");
    await unlink(ws.join("deleted.txt"));
    await mkdir(ws.join("dir"));
    await writeFile(ws.join("dir/untracked.txt"), "untracked\n");

    const agentDiff = await ws.captureAgentDiff();

    expect(agentDiff?.diff).toContain("staged.txt");
    expect(agentDiff?.diff).toContain("+after staged");
    expect(agentDiff?.diff).toContain("unstaged.txt");
    expect(agentDiff?.diff).toContain("+after unstaged");
    expect(agentDiff?.diff).toContain("added.txt");
    expect(agentDiff?.diff).toContain("+added");
    expect(agentDiff?.diff).toContain("deleted.txt");
    expect(agentDiff?.diff).toContain("-before deleted");
    expect(agentDiff?.diff).toContain("+untracked");
    expect(agentDiff?.stats).toEqual({
      filesChanged: 5,
      linesAdded: 4,
      linesRemoved: 3,
    });
    expect(git(ws.rootPath, "status", "--porcelain")).toContain("?? dir/");
  });

  it("captures an unchanged workspace as an empty agent diff", async () => {
    const ws = await gitWorkspace({ "app.txt": "unchanged\n" });

    const agentDiff = await ws.captureAgentDiff();

    expect(agentDiff).toEqual({
      diff: "",
      stats: { filesChanged: 0, linesAdded: 0, linesRemoved: 0 },
    });
  });

  it("creates a bundle and instantiates a workspace from it, honoring subdir and diffs", async () => {
    const spec = await sourceRepo(
      { "root.txt": "root v1\n", "pkg/inner.txt": "inner v1\n" },
      { "root.txt": "root v2\n" },
    );

    const ws = await Workspace.fromGitBundle({
      ...spec,
      bundlePath: await bundle(spec),
      subdir: "pkg",
    });
    cleanups.push(ws.rootPath);

    expect(ws.relativeCwd).toBe("pkg");
    expect(ws.path).toBe(join(ws.rootPath, "pkg"));
    expect(await readFile(join(ws.rootPath, "root.txt"), "utf8")).toBe(
      "root v1\n",
    );

    await writeFile(ws.join("inner.txt"), "inner edited\n");
    expect(ws.referenceDiff?.diff).toContain("root v2");
    expect((await ws.captureAgentDiff())?.diff).toContain("inner edited");
  });

  it.each([
    ["repository", (spec: GitRepoSpec) => Workspace.fromGitRepo(spec)],
    [
      "bundle",
      async (spec: GitRepoSpec) =>
        Workspace.fromGitBundle({ ...spec, bundlePath: await bundle(spec) }),
    ],
  ])(
    "gives the agent only the start commit from a git %s",
    async (_source, create) => {
      const spec = await sourceRepo(
        { "app.txt": "start\n" },
        { "app.txt": "gold\n" },
      );

      const ws = await create(spec);
      cleanups.push(ws.rootPath);

      expect(git(ws.rootPath, "rev-parse", "HEAD")).toBe(spec.startCommit);
      expect(git(ws.rootPath, "for-each-ref", "--format=%(refname)")).toBe(
        "refs/heads/main",
      );
      expect(git(ws.rootPath, "remote")).toBe("");
      expect(() =>
        git(ws.rootPath, "cat-file", "-e", spec.goldCommit),
      ).toThrow();
      expect(ws.referenceDiff?.diff).toContain("+gold");
    },
  );

  it("checks the start commit out on the requested branch", async () => {
    const ws = await gitWorkspace(
      { "app.txt": "start\n" },
      {},
      {
        branch: "trunk",
      },
    );

    expect(git(ws.rootPath, "branch", "--show-current")).toBe("trunk");
  });

  it("rejects capturing the agent diff when git fails", async () => {
    const ws = await gitWorkspace({ "app.txt": "start\n" });
    await writeFile(join(ws.rootPath, ".git", "index"), "corrupt");

    await expect(ws.captureAgentDiff()).rejects.toThrow(/index/);
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

function git(cwd: string, ...args: string[]): string {
  return execFileSync("git", args, { cwd, stdio: "pipe" }).toString().trim();
}

async function gitWorkspace(
  start: Record<string, string>,
  gold: Record<string, string> = {},
  options: Partial<GitRepoSpec> = {},
): Promise<Workspace> {
  const ws = await Workspace.fromGitRepo({
    ...(await sourceRepo(start, gold)),
    ...options,
  });
  cleanups.push(ws.rootPath);
  return ws;
}

async function sourceRepo(
  start: Record<string, string>,
  gold: Record<string, string>,
): Promise<GitRepoSpec> {
  const repo = track(await mkdtemp(join(tmpdir(), "ws-repo-")));
  git(repo, "init", "--initial-branch", "main");
  git(repo, "config", "user.email", "eval@example.com");
  git(repo, "config", "user.name", "Eval");
  const commit = async (files: Record<string, string>, message: string) => {
    for (const [path, content] of Object.entries(files)) {
      await mkdir(dirname(join(repo, path)), { recursive: true });
      await writeFile(join(repo, path), content);
    }
    git(repo, "add", ".");
    git(repo, "commit", "--allow-empty", "-m", message);
    return git(repo, "rev-parse", "HEAD");
  };
  const startCommit = await commit(start, "start");
  const goldCommit = await commit(gold, "gold");
  return { url: `file://${repo}`, startCommit, goldCommit };
}

async function bundle(spec: GitRepoSpec): Promise<string> {
  const bundlePath = join(
    track(await mkdtemp(join(tmpdir(), "ws-bundle-"))),
    "repo.bundle",
  );
  await createGitBundle(spec, bundlePath);
  return bundlePath;
}
