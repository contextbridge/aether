# Aether Sessions

`aether-sessions` owns Aether's persisted session model, JSONL log reader, and
transcript reconstruction helpers.

## Persisted format

A session file is JSON Lines (JSONL):

1. The first non-blank line is a serialized [`SessionMeta`](src/model.rs).
2. Each following non-blank line is a serialized [`SessionEvent`](src/model.rs).
3. Events that are transient while an agent is running may appear in the log,
   but readers expose them separately and only persisted events are used for
   durable session state.

Blank lines are ignored. Malformed event lines are returned as
`SessionLogEntry::Malformed` so a trailing partial write does not hide valid
prior events. Invalid or missing metadata prevents a log from opening.

This is Aether's persisted session format. It is not an ACP wire log: ACP
requests, responses, and notifications use the official ACP types and are
serialized independently.
