import { describe, expect, it } from "vitest";

import {
  formatTranscript,
  judge,
  JudgeResponseSchema,
} from "../../src/evals/index.js";

const baseJudgeInput = {
  task: "do the thing",
  instructions: "be strict",
};

describe("judge", () => {
  it("builds a judge prompt from task, context, instructions, and criteria", () => {
    const result = judge({
      ...baseJudgeInput,
      context: {
        diff: "+added line",
        files: { "notes.txt": "beta\n" },
      },
      criteria: [
        {
          id: "works",
          description: "the task works",
          blocking: true,
          threshold: 0.9,
          weight: 2,
        },
      ],
    });

    expect(result.prompt).toContain("## Instructions\n\nbe strict");
    expect(result.prompt).toContain("## Task");
    expect(result.prompt).toContain(
      "The agent you're evaluating was given this task: <task>do the thing</task>",
    );
    expect(result.prompt).toContain("## Git diff");
    expect(result.prompt).toContain(
      "Git diff produced by the agent you're evaluating: <diff>+added line</diff>",
    );
    expect(result.prompt).toContain("## File Contents");
    expect(result.prompt).toContain("<path>notes.txt</path>");
    expect(result.prompt).toContain("<contents>beta\n</contents>");
    expect(result.prompt).toContain("## Rubric criteria");
    expect(result.prompt).toContain("blocking: true");
    expect(result.prompt).toContain("threshold: 0.9");
    expect(result.prompt).toContain("weight: 2");
    expect(result.prompt).toContain(
      "Return exactly one result for every criterion ID above and no extra criteria.",
    );
    expect(result.prompt).toContain(
      "Respond with ONLY a JSON object matching this schema:",
    );
    expect(result.schema).toBe(JudgeResponseSchema);
  });

  it("normalizes criteria defaults", () => {
    const result = judge({
      ...baseJudgeInput,
      criteria: [{ id: "behavior", description: "does the thing" }],
    });

    expect(result.criteria).toEqual([
      {
        id: "behavior",
        description: "does the thing",
        blocking: true,
        threshold: 1,
        weight: 1,
      },
    ]);
  });

  it("summarizes normalized criterion scores with weights and blockers", () => {
    const result = judge({
      ...baseJudgeInput,
      criteria: [
        {
          id: "behavior",
          description: "does the thing",
          blocking: true,
          threshold: 0.8,
          weight: 3,
        },
        {
          id: "style",
          description: "is maintainable",
          blocking: false,
          threshold: 0.8,
          weight: 1,
        },
      ],
    });

    const summary = result.summarize({
      criteria: [
        { id: "behavior", score: 1, reason: "correct" },
        { id: "style", score: 0.5, reason: "rough" },
      ],
      overall_reason: "mostly good",
    });

    expect(summary.passed).toBe(true);
    expect(summary.score).toBe(0.875);
    expect(summary.reason).toBe(
      "weighted score 0.88; all blockers met; mostly good",
    );
    expect(summary.criteria[0]).toMatchObject({
      id: "behavior",
      score: 1,
    });
    expect(summary.criteria[1]).toMatchObject({
      id: "style",
      score: 0.5,
    });
  });

  it("zeroes the final score when a blocking criterion fails", () => {
    const result = judge({
      ...baseJudgeInput,
      criteria: [
        {
          id: "behavior",
          description: "does the thing",
          blocking: true,
          threshold: 0.8,
          weight: 1,
        },
      ],
    });

    const summary = result.summarize({
      criteria: [{ id: "behavior", score: 0.75, reason: "not quite" }],
      overall_reason: "failed behavior",
    });

    expect(summary.passed).toBe(false);
    expect(summary.score).toBe(0);
    expect(summary.reason).toBe(
      "weighted score 0.75; one or more blockers failed; failed behavior",
    );
  });

  it("rejects invalid judge response criterion sets", () => {
    const result = judge({
      ...baseJudgeInput,
      criteria: [{ id: "behavior", description: "does the thing" }],
    });

    expect(() =>
      result.summarize({ criteria: [], overall_reason: "missing" }),
    ).toThrow(/missing response criterion `behavior`/);
    expect(() =>
      result.summarize({
        criteria: [
          { id: "behavior", score: 1, reason: "ok" },
          { id: "behavior", score: 1, reason: "dupe" },
        ],
        overall_reason: "duplicate",
      }),
    ).toThrow(/duplicate response criterion id `behavior`/);
    expect(() =>
      result.summarize({
        criteria: [
          { id: "behavior", score: 1, reason: "ok" },
          { id: "extra", score: 1, reason: "extra" },
        ],
        overall_reason: "unknown",
      }),
    ).toThrow(/unknown response criterion `extra`/);
  });

  it("formatTranscript joins streamed text chunks and renders tool calls", () => {
    const text = formatTranscript([
      {
        type: "text",
        message_id: "m1",
        chunk: "Hello ",
        is_complete: false,
        model_name: "fake",
      },
      {
        type: "text",
        message_id: "m1",
        chunk: "world",
        is_complete: true,
        model_name: "fake",
      },
      {
        type: "tool_call",
        model_name: "fake",
        request: { id: "c1", name: "bash", arguments: '{"cmd":"ls"}' },
      },
    ]);

    expect(text).toContain("[agent] Hello world");
    expect(text).toContain('[tool-call] bash {"cmd":"ls"}');
  });
});
