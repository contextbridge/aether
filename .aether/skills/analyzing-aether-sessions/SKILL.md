---
name: analyzing-aether-sessions
description: Uses the aether-session-index CLI to inspect local Aether session logs for self-improvement, debugging, tool-use patterns, failures, and optimization opportunities. Use when asked to analyze agent behavior, improve Aether usage, find recurring tool errors, review context usage, or optimize workflows from session history.
agent-invocable: true
---

# Analyze Aether Sessions

Use `aether-session-index` to answer factual questions about past sessions. Aggregate first; only inspect raw events when needed.

## Workflow

1. Build or refresh the local index:
   ```bash
   cargo run -p aether-session-index -- ingest
   ```
   Use `--sessions-dir /path/to/sessions` for non-default logs.

2. Discover the live schema and ready-made example queries:
   ```bash
   cargo run -p aether-session-index -- schema
   ```
   This prints the current tables, columns, views, and a set of example
   queries (most-failing tools, tool error rate, high context usage,
   malformed logs, …). Treat it as the source of truth for what is queryable
   and adapt the examples rather than memorizing column names.

3. Query compact JSON:
   ```bash
   cargo run -p aether-session-index -- query '<sql>'
   ```

## Rules

- The default DB is `$AETHER_HOME/session-index.sqlite`; treat it as a disposable cache and rerun `ingest` after schema or log changes.
- Keep outputs small: prefer `count`, `group by`, `limit`, and CLI defaults.
- Query runs single-statement read-only `select`/`with` SQL.
- For optimization advice, cite the query result pattern first, then give concrete behavior changes.
- Avoid reading raw JSONL unless SQL cannot answer the question.
