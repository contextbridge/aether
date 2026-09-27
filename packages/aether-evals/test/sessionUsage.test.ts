import { describe, expect, it } from "vitest";

import {
  Transcript,
  TranscriptError,
  type AgentEvent,
  type SessionUsageEvent,
  type SessionUsageTotals,
  type TokenUsage,
} from "../src/index.js";

describe("cumulative session usage", () => {
  it("preserves the complete snapshot in a failed transcript", async () => {
    const totals: SessionUsageTotals = {
      tokens: tokenUsage(100, 20),
      estimated_usd: 1,
      estimated_input_usd: 0.25,
      estimated_output_usd: 0.5,
      estimated_cache_read_usd: 0.125,
      estimated_cache_creation_usd: 0.125,
      unpriced_calls: 1,
    };

    const usage: SessionUsageEvent = {
      sequence: 3,
      source: {
        agent_id: "child",
        parent_agent_id: "root",
        task_id: null,
        agent_name: "Explore",
      },
      purpose: "chat",
      model: { provider: null, model_id: null, pricing: null },
      tokens: tokenUsage(10, 2),
      estimated_cost: null,
      totals,
    };

    const failure = new Error("provider failed");
    async function* stream(): AsyncGenerator<AgentEvent> {
      yield { category: "session_usage", event: usage };
      throw failure;
    }

    const error = await Transcript.fromStream(stream()).catch(
      (cause: unknown) => cause,
    );
    expect(error).toBeInstanceOf(TranscriptError);
    if (!(error instanceof TranscriptError)) throw error;
    expect(error.cause).toBe(failure);
    expect(error.transcript.events).toEqual([
      { category: "session_usage", event: usage },
    ]);
  });
});

function tokenUsage(input_tokens: number, output_tokens: number): TokenUsage {
  return {
    input_tokens,
    output_tokens,
    cache_read_tokens: null,
    cache_creation_tokens: null,
    input_audio_tokens: null,
    input_video_tokens: null,
    reasoning_tokens: null,
    output_audio_tokens: null,
    accepted_prediction_tokens: null,
    rejected_prediction_tokens: null,
  };
}
