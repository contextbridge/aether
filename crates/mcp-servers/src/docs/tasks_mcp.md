MCP server for hierarchical task management in deep research agent workflows.

Provides tools for creating, listing, and updating tasks organized into trees with dependency tracking. Tasks carry research context (decisions, facts, next steps, blockers) alongside their status.

# Storage modes

- **Session-scoped** (default via [`TasksMcp::new`]) -- Tasks are stored in a temporary directory that is automatically cleaned up when the server is dropped.
- **Persistent** (via [`TasksMcp::new_persistent`] or `--dir` CLI flag) -- Tasks are stored in a specified directory and survive across sessions.

# Construction

```rust,ignore
use mcp_servers::TasksMcp;

// Session-scoped
let server = TasksMcp::new();

// Persistent
let server = TasksMcp::new_persistent("/my/project/.aether-tasks".into());

// From CLI args (e.g. --dir /path)
let server = TasksMcp::from_args(vec!["--dir".into(), ".".into()]).unwrap();
```

# Tools provided

- **`task_create`** -- Create a new root task tree with a subject and optional description.
- **`task_update`** -- Update a task's status, assignee, dependencies, or research fields.
- **`task_list`** -- List tasks with optional filters by status or assignee.
- **`task_get`** -- Get full details of a specific task by ID.

# See also

- [`Task`](crate::tasks::Task) -- The task data structure.
- [`TaskId`](crate::tasks::TaskId) -- Hierarchical task identifiers.
- [`TaskStatus`](crate::tasks::TaskStatus) -- `Pending`, `InProgress`, `Completed`, `Blocked`.
- [`TaskStore`](crate::tasks::TaskStore) -- Underlying JSONL persistence layer.
