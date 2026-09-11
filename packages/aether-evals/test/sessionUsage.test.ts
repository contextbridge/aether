import { describe, expect, it } from "vitest";

import {
  Transcript,
  TranscriptError,
  type AgentEvent,
  type SessionUsageEvent,
  type SessionUsageTotals,
} from "../src/index.js";

describe("cumulative session usage", () => {
  it("preserves the complete snapshot in a failed transcript", async () => {
    const totals: SessionUsageTotals = {
      tokens: { input_tokens: 100, output_tokens: 20 },
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
        agent_name: "Explore",
      },
      purpose: "chat",
      model: {},
      tokens: { input_tokens: 10, output_tokens: 2 },
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
