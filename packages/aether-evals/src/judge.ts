import { z } from "zod";

import { AetherSdkError } from "@aether-agent/sdk";
import type {
  AgentMessage,
  JudgeCriterionResponse,
  JudgeCriterionSpec,
  JudgeCriterionSummary,
  JudgeRubricResponse,
  JudgeSummary,
} from "@aether-agent/sdk";

export type {
  JudgeCriterionResponse,
  JudgeCriterionSpec,
  JudgeCriterionSummary,
  JudgeRubricResponse,
  JudgeSummary,
};

export interface JudgeInput {
  /** High-level grading instructions. */
  instructions: string;
  /** The task the agent was given. */
  task: string;
  /** Evidence the judge should use when grading the run. */
  context?: JudgeContext;
  /** Ordered rubric criteria to score on a normalized 0.0..=1.0 scale. */
  criteria: JudgeCriterionSpec[];
}

export interface JudgeContext {
  /** The agent transcript, e.g. messages collected from `Agent.run` with `Transcript.fromStream`. */
  transcript?: AgentMessage[];
  /** A workspace diff to grade against. */
  diff?: string;
  /** Final file contents to include, keyed by path. */
  files?: Record<string, string>;
}

export interface Judge {
  prompt: string;
  schema: typeof JudgeResponseSchema;
  criteria: Required<JudgeCriterionSpec>[];
  summarize(response: JudgeRubricResponse): JudgeSummary;
}

export const JudgeCriterionResponseSchema = z.object({
  id: z.string(),
  score: z.number().min(0).max(1),
  reason: z.string(),
});

export const JudgeResponseSchema = z.object({
  criteria: z.array(JudgeCriterionResponseSchema),
  overall_reason: z.string(),
});

/** Compose a judge prompt and deterministic rubric summarizer from structured context. */
export function judge(input: JudgeInput): Judge {
  const criteria = normalizeCriteria(input.criteria);
  const sections: string[] = [
    `## Instructions`,
    `${input.instructions}`,

    `## Task`,
    `The agent you're evaluating was given this task: <task>${input.task}</task>`,
  ];

  if (input.context?.transcript?.length) {
    sections.push(
      `## Agent Transcript`,
      `Transcript of the agent you're evaluating: <transcript>${formatTranscript(input.context.transcript)}</transcript>`,
    );
  }

  if (input.context?.diff) {
    sections.push(
      `## Git diff`,
      `Git diff produced by the agent you're evaluating: <diff>${input.context.diff}</diff>`,
    );
  }

  if (input.context?.files && Object.keys(input.context.files).length > 0) {
    const blocks = Object.entries(input.context.files).map(([path, content]) =>
      [
        `<file>`,
        `<path>${path}</path>`,
        `<contents>${content}</contents>`,
        `</file>`,
      ].join(""),
    );

    sections.push(
      `## File Contents`,
      `Files under evaluation: <files>${blocks.join("\n")}</files>`,
    );
  }

  const rubric = criteria.map(
    (criterion) =>
      `- id: ${criterion.id}\n  blocking: ${criterion.blocking}\n  weight: ${criterion.weight}\n  threshold: ${criterion.threshold}\n  description: ${criterion.description}`,
  );
  sections.push([`## Rubric criteria`, `${rubric.join("\n")}`].join("\n"));
  sections.push(
    [
      "Return exactly one result for every criterion ID above and no extra criteria.",
      "Scores must be normalized numbers from 0.0 to 1.0.",
      "Respond with ONLY a JSON object matching this schema:",
      JSON.stringify(z.toJSONSchema(JudgeResponseSchema), null, 2),
    ].join("\n"),
  );

  return {
    prompt: sections.join("\n\n"),
    schema: JudgeResponseSchema,
    criteria,
    summarize: (response) => summarizeJudgeResponse(criteria, response),
  };
}

/** Render a transcript of streamed `AgentMessage`s as readable lines for a judge prompt. */
export function formatTranscript(messages: AgentMessage[]): string {
  const lines: string[] = [];
  const buffers = new Map<
    string,
    { kind: "agent" | "thinking"; text: string }
  >();

  const flush = (id: string): void => {
    const buffer = buffers.get(id);
    if (buffer && buffer.text.trim())
      lines.push(`[${buffer.kind}] ${buffer.text}`);
    buffers.delete(id);
  };

  for (const message of messages) {
    if (message.type === "text" || message.type === "thought") {
      const kind = message.type === "text" ? "agent" : "thinking";
      const buffer = buffers.get(message.message_id) ?? { kind, text: "" };
      buffer.text += message.chunk;
      buffers.set(message.message_id, buffer);
      if (message.is_complete) flush(message.message_id);
    } else {
      const formatted = messageToString(message);
      if (formatted) lines.push(formatted);
    }
  }

  for (const id of [...buffers.keys()]) flush(id);
  return lines.join("\n");
}

/** Format a single agent message into a readable line for a judge prompt. */
export function messageToString(message: AgentMessage): string {
  switch (message.type) {
    case "tool_call":
      return `[tool-call] ${message.request.name} ${message.request.arguments}`.trimEnd();
    case "tool_result":
      return `[tool-result] ${message.result.name}: ${message.result.result}`;
    case "tool_error":
      return `[tool-error] ${message.error.name}: ${message.error.error}`;
    case "error":
      return `[error] ${message.message}`;
    default:
      return "";
  }
}

function normalizeCriteria(
  criteria: JudgeCriterionSpec[],
): Required<JudgeCriterionSpec>[] {
  if (criteria.length === 0) {
    throw invalidJudgeInput("judge criteria must not be empty");
  }

  const ids = new Set<string>();
  return criteria.map((criterion) => {
    const id = criterion.id.trim();
    if (!id) throw invalidJudgeInput("judge criterion id must not be empty");
    if (ids.has(id))
      throw invalidJudgeInput(`duplicate judge criterion id \`${id}\``);
    ids.add(id);

    const normalized = {
      id,
      description: criterion.description,
      blocking: criterion.blocking ?? true,
      weight: criterion.weight ?? 1,
      threshold: criterion.threshold ?? 1,
    };

    if (!normalized.description.trim()) {
      throw invalidJudgeInput(
        `judge criterion \`${id}\` description must not be empty`,
      );
    }

    if (!Number.isFinite(normalized.weight) || normalized.weight <= 0) {
      throw invalidJudgeInput(
        `judge criterion \`${id}\` weight must be greater than 0`,
      );
    }

    if (
      !Number.isFinite(normalized.threshold) ||
      normalized.threshold < 0 ||
      normalized.threshold > 1
    ) {
      throw invalidJudgeInput(
        `judge criterion \`${id}\` threshold must be between 0.0 and 1.0`,
      );
    }

    return normalized;
  });
}

function summarizeJudgeResponse(
  criteria: Required<JudgeCriterionSpec>[],
  response: JudgeRubricResponse,
): JudgeSummary {
  const responses = new Map<string, JudgeCriterionResponse>();
  for (const criterion of response.criteria) {
    if (responses.has(criterion.id)) {
      throw invalidJudgment(
        `duplicate response criterion id \`${criterion.id}\``,
      );
    }
    responses.set(criterion.id, criterion);
  }

  const summaries: JudgeCriterionSummary[] = [];
  let weightedScore = 0;
  let totalWeight = 0;
  let blockingFailed = false;

  for (const criterion of criteria) {
    const responseCriterion = responses.get(criterion.id);
    if (!responseCriterion) {
      throw invalidJudgment(`missing response criterion \`${criterion.id}\``);
    }
    responses.delete(criterion.id);

    const passed = responseCriterion.score >= criterion.threshold;
    blockingFailed ||= criterion.blocking && !passed;
    weightedScore += responseCriterion.score * criterion.weight;
    totalWeight += criterion.weight;
    summaries.push({
      id: criterion.id,
      description: criterion.description,
      blocking: criterion.blocking,
      weight: criterion.weight,
      threshold: criterion.threshold,
      score: responseCriterion.score,
      passed,
      reason: responseCriterion.reason,
    });
  }

  const unknownId = responses.keys().next().value as string | undefined;
  if (unknownId)
    throw invalidJudgment(`unknown response criterion \`${unknownId}\``);

  weightedScore /= totalWeight;
  const score = blockingFailed ? 0 : weightedScore;
  const reason = blockingFailed
    ? `weighted score ${weightedScore.toFixed(2)}; one or more blockers failed; ${response.overall_reason}`
    : `weighted score ${weightedScore.toFixed(2)}; all blockers met; ${response.overall_reason}`;

  return { passed: !blockingFailed, score, reason, criteria: summaries };
}

function invalidJudgeInput(message: string): AetherSdkError {
  return new AetherSdkError("invalid_options", message);
}

function invalidJudgment(message: string): AetherSdkError {
  return new AetherSdkError("generate_command_failed", message);
}
