-- Backfilled tool arguments only apply on parse, so drop indexed rows to force a full re-ingest.
delete from events;
delete from parse_errors;
delete from session_files;
