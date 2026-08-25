# Aether Sessions

`aether-sessions` owns Aether's persisted session model, JSONL log reader,
transcript reconstruction helpers, and local session storage.

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

## Session storage

[`SessionStore`](src/store/mod.rs) uses `AETHER_HOME/sessions` (or the platform
home fallback) by default. `SessionStore::from_path` is available to callers
that provide an alternate data root, including tests. It owns session JSONL
writes, bounded list and preview reads, relocation, and prompt search.

Prompt search is backed by the derived `prompt-history.jsonl` index in the same
directory. The JSONL session files remain the source of truth; the index is
rewritten atomically when it reaches its 100-entry retention limit or when a
session is relocated. Search uses smart-case matching and reports UTF-8 byte
offsets into the original prompt.

## Analytics

The optional `analytics` feature owns the `SQLite` projection of persisted
session files. JSONL remains the source of truth; the `SQLite` database is a
derived cache that supports concurrent ingest, replacement and pruning, safe
read-only queries, and documented schema examples.

```bash
cargo run -p aether-session-index --features aether-sessions/analytics -- ingest
cargo sqlx prepare --check -- --features analytics
```

The unpublished `aether-session-index` crate is a thin CLI over this feature.
