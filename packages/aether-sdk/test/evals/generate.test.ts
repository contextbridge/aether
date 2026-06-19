import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { z } from "zod";

import { generate } from "../../src/evals/index.js";

/**
 * A stand-in for the real `aether generate` binary. It validates argv, reads the prompt from stdin,
 * captures `{ argv, prompt, model, system }` to `FAKE_GENERATE_CAPTURE` when set, and prints a JSON
 * response. Behavior is steered by env vars so a single fake covers every case.
 */
const FAKE_GENERATE = `#!/usr/bin/env node
import { writeFileSync } from "node:fs";

const argv = process.argv.slice(2);
if (argv[0] !== "generate" || !argv.includes("--model") || !argv.includes("--prompt-file") || !argv.includes("-") || !argv.includes("--output")) {
  process.stderr.write("unexpected argv: " + JSON.stringify(argv) + "\\n");
  process.exit(1);
}

const chunks = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const prompt = Buffer.concat(chunks).toString("utf8");

const model = argv[argv.indexOf("--model") + 1];
const systemIdx = argv.indexOf("--system");
const system = systemIdx === -1 ? null : argv[systemIdx + 1];

if (process.env.FAKE_GENERATE_CAPTURE) {
  writeFileSync(process.env.FAKE_GENERATE_CAPTURE, JSON.stringify({ argv, prompt, model, system }));
}

if (process.env.FAKE_GENERATE_EXIT) {
  process.stderr.write(process.env.FAKE_GENERATE_STDERR ?? "boom");
  process.exit(Number(process.env.FAKE_GENERATE_EXIT));
}

const stdout = process.env.FAKE_GENERATE_RAW_STDOUT ?? JSON.stringify({ text: process.env.FAKE_GENERATE_RESPONSE ?? "default response", model });
process.stdout.write(stdout + "\\n", () => process.exit(0));
`;

describe("generate", () => {
  let scratch: string;
  let fakeBinary: string;

  beforeEach(async () => {
    scratch = await mkdtemp(join(tmpdir(), "generate-test-"));
    fakeBinary = join(scratch, "fake-aether.mjs");
    await writeFile(fakeBinary, FAKE_GENERATE, { mode: 0o755 });
  });

  afterEach(async () => {
    await rm(scratch, { recursive: true, force: true });
  });

  const baseEnv = (extra: Record<string, string>) => ({
    ...process.env,
    ...extra,
  });

  it("returns the model's response text", async () => {
    const result = await generate("say hi", {
      binaryPath: fakeBinary,
      model: "anthropic:claude-sonnet-4-5",
      env: baseEnv({ FAKE_GENERATE_RESPONSE: "hello world" }),
    });

    expect(result.text).toBe("hello world");
  });

  it("sends the model, prompt on stdin, and json output flag", async () => {
    const capture = join(scratch, "capture.json");
    await generate("the prompt body", {
      binaryPath: fakeBinary,
      model: "anthropic:claude-sonnet-4-5",
      system: "be terse",
      env: baseEnv({ FAKE_GENERATE_CAPTURE: capture }),
    });

    const captured = JSON.parse(await readFile(capture, "utf8"));
    expect(captured.model).toBe("anthropic:claude-sonnet-4-5");
    expect(captured.prompt).toBe("the prompt body");
    expect(captured.system).toBe("be terse");
    expect(captured.argv).toContain("--output");
    expect(captured.argv).toContain("json");
  });

  it("passes model settings and reasoning effort to the CLI", async () => {
    const capture = join(scratch, "capture.json");
    await generate("grade deterministically", {
      binaryPath: fakeBinary,
      model: "anthropic:claude-sonnet-4-5",
      temperature: 0,
      topP: 0.5,
      maxTokens: 64,
      reasoningEffort: "high",
      env: baseEnv({ FAKE_GENERATE_CAPTURE: capture }),
    });

    const captured = JSON.parse(await readFile(capture, "utf8"));
    expect(captured.argv).toEqual(
      expect.arrayContaining([
        "--temperature",
        "0",
        "--top-p",
        "0.5",
        "--max-tokens",
        "64",
        "--reasoning-effort",
        "high",
      ]),
    );
  });

  it("returns typed JSON when a schema is provided", async () => {
    const capture = join(scratch, "capture.json");
    const verdict = await generate("grade the run", {
      binaryPath: fakeBinary,
      model: "anthropic:claude-sonnet-4-5",
      schema: z.object({ passed: z.boolean(), reason: z.string() }),
      env: baseEnv({
        FAKE_GENERATE_CAPTURE: capture,
        FAKE_GENERATE_RESPONSE: '{"passed":true,"reason":"ok"}',
      }),
    });

    expect(verdict).toEqual({ passed: true, reason: "ok" });
    const captured = JSON.parse(await readFile(capture, "utf8"));
    expect(captured.prompt).toContain("grade the run");
    expect(captured.prompt).toContain("Respond with ONLY valid JSON");
  });

  it("rejects JSON responses that do not match the schema", async () => {
    await expect(
      generate("grade the run", {
        binaryPath: fakeBinary,
        model: "anthropic:claude-sonnet-4-5",
        schema: z.object({ passed: z.boolean(), reason: z.string() }),
        env: baseEnv({ FAKE_GENERATE_RESPONSE: '{"passed":"yes"}' }),
      }),
    ).rejects.toThrow(/did not match schema/);
  });

  it("throws invalid_options when no model is given", async () => {
    const saved = process.env.AETHER_LLM_MODEL;
    delete process.env.AETHER_LLM_MODEL;
    try {
      await expect(generate("p", { binaryPath: fakeBinary })).rejects.toThrow(
        /No model provided/,
      );
    } finally {
      if (saved !== undefined) process.env.AETHER_LLM_MODEL = saved;
    }
  });

  it("rejects with generate_command_failed on a non-zero exit", async () => {
    await expect(
      generate("p", {
        binaryPath: fakeBinary,
        model: "anthropic:claude-sonnet-4-5",
        env: baseEnv({
          FAKE_GENERATE_EXIT: "1",
          FAKE_GENERATE_STDERR: "model unavailable",
        }),
      }),
    ).rejects.toThrow(/aether generate exited with code 1|model unavailable/);
  });

  it("rejects when stdout is not JSON", async () => {
    await expect(
      generate("p", {
        binaryPath: fakeBinary,
        model: "anthropic:claude-sonnet-4-5",
        env: baseEnv({ FAKE_GENERATE_RAW_STDOUT: "this is not json" }),
      }),
    ).rejects.toThrow(/Failed to parse aether generate output as JSON/);
  });
});
