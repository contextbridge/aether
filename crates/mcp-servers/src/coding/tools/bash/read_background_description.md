Retrieves output from a background bash shell (started with `run_in_background: true`).

- `bash_id` — **required**, shell ID returned from the original bash call
- Returns only new output since the last check, along with shell status (running/completed/failed)
- Supports optional regex filtering to show only matching lines
