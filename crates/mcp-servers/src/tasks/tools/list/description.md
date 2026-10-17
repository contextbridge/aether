Lists tasks in ID order. Filters are optional and combine.

## Usage

```json
{}
{"assignee": "worker-1"}
{"status": "in_progress"}
{"tree_id": "at-a1b2c3d4"}
{"ready_only": true}
{"assignee": "worker-1", "status": "pending"}
```

No filters → returns all active tasks.
