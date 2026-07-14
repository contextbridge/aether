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

Fixed internal queries use SQLx compile-time checking. If the schema or checked queries change, refresh the offline metadata:

```bash
tmpdb=$(mktemp /tmp/aether-session-index.XXXXXX)
cd crates/aether-session-index
DATABASE_URL="sqlite://$tmpdb" cargo sqlx migrate run
DATABASE_URL="sqlite://$tmpdb" cargo sqlx prepare
```

Check metadata with:

```bash
DATABASE_URL="sqlite://$tmpdb" cargo sqlx prepare --check
```
