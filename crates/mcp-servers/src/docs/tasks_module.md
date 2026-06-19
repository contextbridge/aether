Task management for deep research agent workflows.

Provides a standalone MCP server for hierarchical task tracking with support for:
- **Task trees** -- Root tasks with subtasks, organized by subject
- **Dependencies** -- Tasks can declare blockers; [`Task::is_ready`] checks readiness
- **Multi-agent coordination** -- Assignee field for distributing work across sub-agents
- **Persistent storage** -- Git-friendly JSONL files, one per root task tree

# Key types

- [`TasksMcp`] -- The MCP server exposing task tools
- [`Task`] -- A single task with status, assignee, dependencies, and research fields
- [`TaskId`] -- Hierarchical identifier (`at-{hash}` for roots, `at-{hash}.{n}` for subtasks)
- [`TaskStatus`] -- `Pending`, `InProgress`, `Completed`, `Blocked`
- [`TaskStore`] -- JSONL file storage with in-memory index
- [`TaskIndex`] -- In-memory index for fast queries by status, assignee, or ID

# Usage

```rust,ignore
use mcp_servers::TasksMcp;
use mcp_utils::ServiceExt;

// Session-scoped (temp dir, auto-cleanup)
let server = TasksMcp::new().into_dyn();

// Persistent across sessions
let server = TasksMcp::new_persistent(".aether-tasks".into()).into_dyn();
```
