import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { Workspace } from "../src/workspace.js";

describe("createWorkspace", () => {
  it("creates an empty workspace", async () => {
    await using ws = await Workspace.empty();

    expect(existsSync(ws.rootPath)).toBe(true);
    expect(ws.path).toBe(ws.rootPath);
  });

  it("creates a populated workspace in a subdir", async () => {
    await using ws = await Workspace.create(
      async (rootPath) => {
        await mkdir(join(rootPath, "pkg"));
        await writeFile(join(rootPath, "pkg", "inner.txt"), "inner\n");
      },
      { subdir: "pkg" },
    );

    expect(ws.relativeCwd).toBe("pkg");
    expect(ws.path).toBe(join(ws.rootPath, "pkg"));
    expect(await readFile(ws.join("inner.txt"), "utf8")).toBe("inner\n");
  });

  it("removes the workspace when its subdir does not exist", async () => {
    let root = "";

    await expect(
      Workspace.create(
        async (rootPath) => {
          root = rootPath;
        },
        { subdir: "missing" },
      ),
    ).rejects.toMatchObject({ code: "configuration_error" });
    expect(existsSync(root)).toBe(false);
  });

  it("writes inline files, including nested paths", async () => {
    await using ws = await Workspace.fromFiles({
      "notes.txt": "alpha\n",
      "dir/deep.txt": "deep",
    });

    expect(await readFile(join(ws.path, "notes.txt"), "utf8")).toBe("alpha\n");
    expect(await readFile(join(ws.path, "dir/deep.txt"), "utf8")).toBe("deep");
  });

  it("rejects inline file paths that escape the workspace root", async () => {
    await expect(Workspace.fromFiles({ "../evil.txt": "x" })).rejects.toThrow(
      /escapes the workspace root/,
    );
  });

  it("copies a directory relative to the base dir", async () => {
    const baseDir = await mkdtemp(join(tmpdir(), "ws-base-"));
    await using _baseDir = removing(baseDir);
    await writeFile(join(baseDir, "fixture.txt"), "from dir");

    await using ws = await Workspace.fromDir(".", baseDir);

    expect(await readFile(join(ws.path, "fixture.txt"), "utf8")).toBe(
      "from dir",
    );
  });

  it("persists a workspace and leaves cleanup with the caller", async () => {
    const ws = await Workspace.fromFiles({ "notes.txt": "keep\n" });
    const retained = ws.persist();
    await using _retained = removing(retained.rootPath);

    await ws.cleanup();

    expect(retained.path).toBe(ws.path);
    expect(existsSync(retained.rootPath)).toBe(true);
    expect(await readFile(ws.join("notes.txt"), "utf8")).toBe("keep\n");
  });
});

function removing(path: string): AsyncDisposable {
  return {
    [Symbol.asyncDispose]: () => rm(path, { recursive: true, force: true }),
  };
}
