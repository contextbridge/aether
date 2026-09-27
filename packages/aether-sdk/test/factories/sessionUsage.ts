import { Factory } from "fishery";

import type { SessionUsageEvent } from "../../src/generated/types.js";

export const sessionUsageFactory = Factory.define<SessionUsageEvent>(() => ({
  sequence: 10,
  source: {
    agent_id: "agent-child",
    parent_agent_id: "agent-root",
    task_id: "task-2",
    agent_name: "Explore",
  },
  purpose: "chat",
  model: {
    provider: "anthropic",
    model_id: "claude-sonnet-4-5",
    pricing: {
      input_per_million: 10,
      output_per_million: 20,
      cache_read_per_million: 30,
      cache_write_per_million: 40,
    },
  },
  tokens: {
    input_tokens: 10,
    output_tokens: 20,
    cache_read_tokens: 30,
    cache_creation_tokens: null,
    input_audio_tokens: null,
    input_video_tokens: null,
    reasoning_tokens: 40,
    output_audio_tokens: null,
    accepted_prediction_tokens: null,
    rejected_prediction_tokens: null,
  },
  estimated_cost: {
    input_usd: 10,
    output_usd: 20,
    cache_read_usd: 30,
    cache_creation_usd: 40,
    total_usd: 100,
  },
  totals: {
    tokens: {
      input_tokens: 100,
      output_tokens: 200,
      cache_read_tokens: 300,
      cache_creation_tokens: null,
      input_audio_tokens: null,
      input_video_tokens: null,
      reasoning_tokens: null,
      output_audio_tokens: null,
      accepted_prediction_tokens: null,
      rejected_prediction_tokens: null,
    },
    estimated_usd: 400,
    estimated_input_usd: 40,
    estimated_output_usd: 80,
    estimated_cache_read_usd: 120,
    estimated_cache_creation_usd: 160,
    unpriced_calls: 10,
  },
}));
