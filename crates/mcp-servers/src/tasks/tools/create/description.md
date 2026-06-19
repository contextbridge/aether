Creates a new task or subtask for tracking work.

Tasks are organized into trees: root tasks can have subtasks, and subtasks can have dependencies.

## Usage

```json
{"title": "Research multi-agent systems", "description": "Investigate patterns", "assignee": "orchestrator"}
{"title": "Analyze paper X", "parent_id": "at-a1b2c3d4", "assignee": "worker-1"}
```

- `title` — **required**
- `parent_id` — creates subtask under this task (omit for root task)
- `description` — detailed description (markdown)
- `assignee` — agent/worker to assign
- `deps` — task IDs that must complete before this can start
