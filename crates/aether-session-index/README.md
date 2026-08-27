# aether-session-index

Maintainer-facing CLI for indexing and querying Aether session logs. This crate is not published.

## Ingest

```bash
cargo run -p aether-session-index -- ingest
cargo run -p aether-session-index -- ingest --sessions-dir /path/to/sessions
```

## Query

```bash
cargo run -p aether-session-index -- query \
  'select tool_name, count(*) from tool_errors group by tool_name order by 2 desc'
```

## Schema

```bash
cargo run -p aether-session-index -- schema
```

## SQLx query metadata

The analytics implementation and its checked queries live in `aether-sessions`.
Use the workspace recipe to verify or refresh the metadata:

```bash
just sqlx-check
```
