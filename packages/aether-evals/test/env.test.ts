import { describe, expect, it } from "vitest";

import { CONTAINER_AETHER_HOME, evalEnvVars } from "../src/index.js";

const HOST_ENV = {
  ANTHROPIC_API_KEY: "anthropic-key",
  OPENAI_API_KEY: "openai-key",
  OLLAMA_HOST: "http://gpu-box:11434",
  AETHER_CREDENTIALS_PASSWORD: "hunter2",
  AETHER_HOME: "/host/aether",
  AWS_ACCESS_KEY_ID: "id",
  AWS_REGION: "us-east-1",
};

describe("evalEnvVars", () => {
  it("forwards only the eval model's provider credentials", () => {
    expect(evalEnvVars("anthropic:claude-opus-4-6", HOST_ENV)).toEqual({
      ANTHROPIC_API_KEY: "anthropic-key",
      AETHER_HOME: CONTAINER_AETHER_HOME,
    });
    expect(evalEnvVars("ollama:llama3", HOST_ENV)).toEqual({
      OLLAMA_HOST: "http://gpu-box:11434",
      AETHER_HOME: CONTAINER_AETHER_HOME,
    });
  });

  it("forwards every alloy member's credentials", () => {
    expect(
      Object.keys(
        evalEnvVars("anthropic:claude-opus-4-6,openai:gpt-5", HOST_ENV),
      ).sort(),
    ).toEqual(["AETHER_HOME", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"]);
  });

  it("forwards bedrock credentials", () => {
    expect(
      evalEnvVars("bedrock:anthropic.claude-opus-4-6-v1", HOST_ENV),
    ).toEqual({
      AWS_ACCESS_KEY_ID: "id",
      AWS_REGION: "us-east-1",
      AETHER_HOME: CONTAINER_AETHER_HOME,
    });
  });

  it("rejects an unknown provider", () => {
    expect(() => evalEnvVars("nope:model", HOST_ENV)).toThrow(
      /unknown provider/,
    );
  });
});
