import { describe, expect, it } from "vitest";

import { DockerImage } from "../../src/evals/DockerImage.js";
import {
  extractToolCalls,
  isTerminalMessage,
} from "../../src/evals/transcript.js";
import type { AgentMessage } from "../../src/generated/eval-types.js";

describe("DockerImage.parse", () => {
  it("splits name and tag", () => {
    expect(DockerImage.parse("aether-sandbox:dev")).toEqual(
      new DockerImage("aether-sandbox", "dev"),
    );
  });

  it("keeps a registry path and reads the trailing tag", () => {
    expect(DockerImage.parse("ghcr.io/org/aether:sha")).toEqual(
      new DockerImage("ghcr.io/org/aether", "sha"),
    );
  });

  it("defaults the tag to latest", () => {
    expect(DockerImage.parse("aether-sandbox")).toEqual(
      new DockerImage("aether-sandbox"),
    );
  });

  it("formats image references", () => {
    expect(new DockerImage("aether-sandbox", "dev").toString()).toBe(
      "aether-sandbox:dev",
    );
  });

  it("attaches Dockerfile build settings", () => {
    expect(
      DockerImage.fromDockerfile("aether-sandbox:dev", {
        context: "/repo",
        dockerfile: "Dockerfile.eval",
        buildArgs: { AETHER_VERSION: "test" },
      }),
    ).toEqual(
      new DockerImage("aether-sandbox", "dev", {
        context: "/repo",
        dockerfile: "Dockerfile.eval",
        buildArgs: { AETHER_VERSION: "test" },
      }),
    );
  });

  it("rejects malformed references", () => {
    expect(() => DockerImage.parse(":latest")).toThrow(/invalid Docker image/);
    expect(() => DockerImage.parse("aether:")).toThrow(/invalid Docker image/);
  });
});

describe("isTerminalMessage", () => {
  it("is true for done/error/cancelled and false otherwise", () => {
    expect(isTerminalMessage({ type: "done" })).toBe(true);
    expect(isTerminalMessage({ type: "error", message: "x" })).toBe(true);
    expect(isTerminalMessage({ type: "cancelled", message: "x" })).toBe(true);
    expect(
      isTerminalMessage({
        type: "text",
        message_id: "m",
        chunk: "hi",
        is_complete: true,
        model_name: "x",
      }),
    ).toBe(false);
  });
});

describe("extractToolCalls", () => {
  it("derives calls from tool_result and tool_error, parsing arguments", () => {
    const messages: AgentMessage[] = [
      {
        type: "tool_result",
        model_name: "x",
        result: {
          id: "1",
          name: "bash",
          arguments: '{"cmd":"pwd"}',
          result: "ok",
        },
      },
      {
        type: "tool_error",
        model_name: "x",
        error: { id: "2", name: "read", error: "missing" },
      },
      {
        type: "text",
        message_id: "m",
        chunk: "hi",
        is_complete: true,
        model_name: "x",
      },
    ];

    expect(extractToolCalls(messages)).toEqual([
      {
        name: "bash",
        arguments: { cmd: "pwd" },
        rawArguments: '{"cmd":"pwd"}',
      },
      { name: "read", arguments: undefined, rawArguments: "" },
    ]);
  });
});
