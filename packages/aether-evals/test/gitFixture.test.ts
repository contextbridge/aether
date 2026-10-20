import { mkdir, readFile, unlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { GitFixture } from "../src/index.js";
import { git, gitRepo, type GitRepoBuilder } from "./gitRepo.js";

describe("GitFixture", () => {
  it("checks out the start commit, honoring a subdir", async () => {
    await using repo = gitRepo().start({
      "root.txt": "root\n",
      "pkg/inner.txt": "inner\n",
    });

    const ws = await repo.workspace({ subdir: "pkg" });

    expect(ws.relativeCwd).toBe("pkg");
    expect(ws.path).toBe(join(ws.rootPath, "pkg"));
    expect(await readFile(join(ws.rootPath, "root.txt"), "utf8")).toBe(
      "root\n",
    );
    expect(await readFile(join(ws.path, "inner.txt"), "utf8")).toBe("inner\n");
  });

  it("captures agent and reference git diffs with stats", async () => {
    await using repo = gitRepo()
      .start({ "app.txt": "before\n" })
      .gold({ "app.txt": "gold\n", "gold.txt": "added\n" });
    const { fixture, ws } = await repo.checkout();
    await writeFile(ws.join("app.txt"), "agent\n");

    const agentDiff = await fixture.agentDiff(ws);

    expect(agentDiff.diff).toContain("+agent");
    expect(agentDiff.stats).toMatchObject({
      filesChanged: 1,
      linesAdded: 1,
      linesRemoved: 1,
    });
    expect(fixture.referenceDiff.diff).toContain("+gold");
    expect(fixture.referenceDiff.stats.filesChanged).toBe(2);
  });

  it("captures committed agent changes from the start commit", async () => {
    await using repo = gitRepo().start({ "app.txt": "before\n" });
    const { fixture, ws } = await repo.checkout();
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

    const agentDiff = await fixture.agentDiff(ws);

    expect(agentDiff.diff).toContain("+agent committed");
    expect(agentDiff.stats).toMatchObject({
      filesChanged: 1,
      linesAdded: 1,
      linesRemoved: 1,
    });
  });

  it("captures staged, unstaged, added, deleted, and untracked changes", async () => {
    await using repo = gitRepo().start({
      "staged.txt": "before staged\n",
      "unstaged.txt": "before unstaged\n",
      "deleted.txt": "before deleted\n",
    });
    const { fixture, ws } = await repo.checkout();

    await writeFile(ws.join("staged.txt"), "after staged\n");
    await writeFile(ws.join("added.txt"), "added\n");
    git(ws.rootPath, "add", "staged.txt", "added.txt");
    await writeFile(ws.join("unstaged.txt"), "after unstaged\n");
    await unlink(ws.join("deleted.txt"));
    await mkdir(ws.join("dir"));
    await writeFile(ws.join("dir/untracked.txt"), "untracked\n");

    const agentDiff = await fixture.agentDiff(ws);

    expect(agentDiff.diff).toContain("staged.txt");
    expect(agentDiff.diff).toContain("+after staged");
    expect(agentDiff.diff).toContain("unstaged.txt");
    expect(agentDiff.diff).toContain("+after unstaged");
    expect(agentDiff.diff).toContain("added.txt");
    expect(agentDiff.diff).toContain("+added");
    expect(agentDiff.diff).toContain("deleted.txt");
    expect(agentDiff.diff).toContain("-before deleted");
    expect(agentDiff.diff).toContain("+untracked");
    expect(agentDiff.stats).toEqual({
      filesChanged: 5,
      linesAdded: 4,
      linesRemoved: 3,
    });
    expect(git(ws.rootPath, "status", "--porcelain")).toContain("?? dir/");
  });

  it("captures an unchanged workspace as an empty agent diff", async () => {
    await using repo = gitRepo().start({ "app.txt": "unchanged\n" });
    const { fixture, ws } = await repo.checkout();

    const agentDiff = await fixture.agentDiff(ws);

    expect(agentDiff).toEqual({
      diff: "",
      stats: { filesChanged: 0, linesAdded: 0, linesRemoved: 0 },
    });
  });

  it("round-trips through a bundle, honoring subdir and diffs", async () => {
    await using repo = gitRepo()
      .start({ "root.txt": "root v1\n", "pkg/inner.txt": "inner v1\n" })
      .gold({ "root.txt": "root v2\n" });
    const fixture = await repo.fixture({
      ...(await repo.bundleSpec()),
      subdir: "pkg",
    });

    await using ws = await fixture.workspace();

    expect(ws.relativeCwd).toBe("pkg");
    expect(ws.path).toBe(join(ws.rootPath, "pkg"));
    expect(await readFile(join(ws.rootPath, "root.txt"), "utf8")).toBe(
      "root v1\n",
    );
    await writeFile(ws.join("inner.txt"), "inner edited\n");
    expect(fixture.referenceDiff.diff).toContain("root v2");
    expect((await fixture.agentDiff(ws)).diff).toContain("inner edited");
  });

  it("round-trips a bundle through paths relative to the caller", async () => {
    await using repo = gitRepo()
      .start({ "app.txt": "start\n" })
      .gold({ "app.txt": "gold\n" });
    const bundle = join(await repo.tempDir("bundle-", "."), "repo.bundle");

    const spec = await repo.bundleSpec(bundle);
    const fixture = await repo.fixture(spec);
    await using ws = await fixture.workspace();

    expect(git(ws.rootPath, "rev-parse", "HEAD")).toBe(spec.startCommit);
    expect(fixture.referenceDiff.diff).toContain("+gold");
  });

  it.each([
    ["repository", (repo: GitRepoBuilder) => repo.spec()],
    ["bundle", (repo: GitRepoBuilder) => repo.bundleSpec()],
  ])(
    "gives the agent only the start commit from a git %s",
    async (_source, toSpec) => {
      await using repo = gitRepo()
        .start({ "app.txt": "start\n" })
        .gold({ "app.txt": "gold\n" });
      const spec = await toSpec(repo);
      const fixture = await repo.fixture(spec);

      await using ws = await fixture.workspace();

      expect(git(ws.rootPath, "rev-parse", "HEAD")).toBe(spec.startCommit);
      expect(git(ws.rootPath, "for-each-ref", "--format=%(refname)")).toBe(
        "refs/heads/main",
      );
      expect(git(ws.rootPath, "remote")).toBe("");
      expect(() =>
        git(ws.rootPath, "cat-file", "-e", spec.goldCommit),
      ).toThrow();
      expect(fixture.referenceDiff.diff).toContain("+gold");
    },
  );

  it("checks each workspace out fresh", async () => {
    await using repo = gitRepo().start({ "app.txt": "start\n" });
    const { fixture, ws: first } = await repo.checkout();
    await writeFile(first.join("app.txt"), "agent\n");

    await using second = await fixture.workspace();

    expect(second.rootPath).not.toBe(first.rootPath);
    expect(await readFile(second.join("app.txt"), "utf8")).toBe("start\n");
  });

  it("checks the start commit out on the requested branch", async () => {
    await using repo = gitRepo().start({ "app.txt": "start\n" });

    const ws = await repo.workspace({ branch: "trunk" });

    expect(git(ws.rootPath, "branch", "--show-current")).toBe("trunk");
  });

  it("leaves excluded paths out of the reference and agent diffs", async () => {
    await using repo = gitRepo()
      .start({ "app.txt": "start\n", "pkg/pnpm-lock.yaml": "lock: 1\n" })
      .gold({ "app.txt": "gold\n", "pkg/pnpm-lock.yaml": "lock: 2\n" });
    const { fixture, ws } = await repo.checkout({
      diffExclude: ["*pnpm-lock.yaml"],
    });
    await writeFile(ws.join("app.txt"), "agent\n");
    await writeFile(ws.join("pkg/pnpm-lock.yaml"), "lock: 3\n");

    const agentDiff = await fixture.agentDiff(ws);

    expect(fixture.referenceDiff.diff).toContain("+gold");
    expect(fixture.referenceDiff.diff).not.toContain("pnpm-lock.yaml");
    expect(agentDiff.diff).toContain("+agent");
    expect(agentDiff.diff).not.toContain("pnpm-lock.yaml");
  });

  it("keeps files tracked despite .gitignore out of the agent diff", async () => {
    await using repo = gitRepo().start({
      ".gitignore": "*.env\n",
      "app.env": "start\n",
    });
    const { fixture, ws } = await repo.checkout();

    expect((await fixture.agentDiff(ws)).diff).toBe("");
  });

  it("rebuilds a deleted index when capturing the agent diff", async () => {
    await using repo = gitRepo().start({ "app.txt": "start\n" });
    const { fixture, ws } = await repo.checkout();
    await writeFile(ws.join("app.txt"), "agent\n");
    await unlink(join(ws.rootPath, ".git", "index"));

    const agentDiff = await fixture.agentDiff(ws);

    expect(agentDiff.diff).toContain("+agent");
    expect(agentDiff.stats.filesChanged).toBe(1);
  });

  it("keeps files tracked despite .gitignore when rebuilding a deleted index", async () => {
    await using repo = gitRepo().start({
      ".gitignore": "*.env\n",
      "app.env": "start\n",
    });
    const { fixture, ws } = await repo.checkout();
    await unlink(join(ws.rootPath, ".git", "index"));

    expect((await fixture.agentDiff(ws)).diff).toBe("");
  });

  it("rejects capturing the agent diff when git fails", async () => {
    await using repo = gitRepo().start({ "app.txt": "start\n" });
    const { fixture, ws } = await repo.checkout();
    await writeFile(join(ws.rootPath, ".git", "index"), "corrupt");

    await expect(fixture.agentDiff(ws)).rejects.toThrow(/index/);
  });

  it("rejects a missing bundle file", async () => {
    await expect(
      GitFixture.fetch({
        source: { bundle: join(tmpdir(), "does-not-exist.bundle") },
        startCommit: "abc",
        goldCommit: "def",
      }),
    ).rejects.toThrow(/git bundle file does not exist/);
  });
});
