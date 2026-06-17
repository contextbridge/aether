import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { runEval } from "../../src/evals/index.js";

/**
 * A stand-in for the real `aether eval --spec-file -` binary: it parses the spec from
 * stdin, writes the inline workspace files to a fresh temp dir, and prints a passing
 * `EvalOutcome` whose `retainedWorkspace` points at that dir. Lets us exercise the `runEval` helper
 * without Docker.
 */
const FAKE_AETHER = `#!/usr/bin/env node
import { mkdirSync, writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

if (!process.argv.includes("--spec-file") || !process.argv.includes("-") || !process.argv.includes("--retain-workspace")) {
  process.stderr.write("unexpected argv: " + JSON.stringify(process.argv) + "\\n");
  process.exit(1);
}

const chunks = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const spec = JSON.parse(Buffer.concat(chunks).toString("utf8"));
const workspace = mkdtempSync(join(tmpdir(), "fake-eval-workspace-"));
for (const [rel, content] of Object.entries(spec.task?.workspace?.files ?? {})) {
  const path = join(workspace, rel);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, content);
}
const outcome = {
  name: spec.name,
  passed: true,
  failures: [],
  toolCalls: [
    {
      name: "weather__get_current",
      arguments: { city: "Tokyo" },
      rawArguments: '{"city":"Tokyo"}',
    },
  ],
  retainedWorkspace: { rootPath: workspace, path: workspace },
};
process.stdout.write(JSON.stringify(outcome) + "\\n", () => process.exit(0));
`;

const SPEC = {
  docker: { image: "sandbox:latest" },
  agent: { command: ["node", "/app/eval-agent.js"] },
  name: "edit-notes",
  task: {
    prompt: "do the thing",
    workspace: { files: { "notes.txt": "alpha\n" } },
  },
};

describe("runEval", () => {
  let scratch: string;
  let fakeBinary: string;

  beforeEach(async () => {
    scratch = await mkdtemp(join(tmpdir(), "runEval-test-"));
    fakeBinary = join(scratch, "fake-aether.mjs");
    await writeFile(fakeBinary, FAKE_AETHER, { mode: 0o755 });
  });

  afterEach(async () => {
    await rm(scratch, { recursive: true, force: true });
  });

  it("returns the parsed outcome and a workspace handle with the written files", async () => {
    const result = await runEval(SPEC, { binaryPath: fakeBinary });

    expect(result.passed).toBe(true);
    expect(result.name).toBe("edit-notes");
    expect(result.toolCalls).toEqual([
      {
        name: "weather__get_current",
        arguments: { city: "Tokyo" },
        rawArguments: '{"city":"Tokyo"}',
      },
    ]);
    expect(existsSync(result.workspace.path)).toBe(true);
    expect(result.workspace.rootPath).toBe(result.workspace.path);
    expect(
      await readFile(join(result.workspace.path, "notes.txt"), "utf8"),
    ).toBe("alpha\n");

    await result.workspace.cleanup();
    expect(existsSync(result.workspace.path)).toBe(false);
  });

  it("removes the workspace at the end of an `await using` scope", async () => {
    let workspacePath: string;
    {
      await using result = await runEval(SPEC, { binaryPath: fakeBinary });
      workspacePath = result.workspace.path;
      expect(existsSync(workspacePath)).toBe(true);
    }
    expect(existsSync(workspacePath)).toBe(false);
  });

  it("retains the workspace on dispose when keepWorkspace is set", async () => {
    let workspacePath: string;
    {
      await using result = await runEval(SPEC, {
        binaryPath: fakeBinary,
        keepWorkspace: true,
      });
      workspacePath = result.workspace.path;
    }
    expect(existsSync(workspacePath)).toBe(true);
    await rm(workspacePath, { recursive: true, force: true });
  });

  it("throws invalid_options when the spec carries an `expect` field", async () => {
    await expect(
      runEval(
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        { ...SPEC, expect: { files: { "notes.txt": "beta\n" } } } as any,
        { binaryPath: fakeBinary },
      ),
    ).rejects.toThrow(/`expect` is not allowed/);
  });
});
