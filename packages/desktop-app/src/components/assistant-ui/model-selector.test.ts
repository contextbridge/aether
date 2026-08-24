import { describe, expect, it } from "vitest";
import { modelSelectorFilter } from "./model-selector";

describe("modelSelectorFilter", () => {
  it("matches model ids and keywords case-insensitively", () => {
    expect(
      modelSelectorFilter("openai:gpt-5.6-luna", "Luna", ["OpenAI"]),
    ).toBeGreaterThan(0);
  });

  it("rejects values that do not contain the search text", () => {
    expect(
      modelSelectorFilter("openai:gpt-5.6-luna", "Claude", ["OpenAI"]),
    ).toBe(0);
  });
});
