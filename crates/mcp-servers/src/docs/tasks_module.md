Task management for deep research agent workflows.

Provides a standalone MCP server for hierarchical task tracking with support for:
- **Task trees** -- Root tasks with subtasks, organized by subject
- **Dependencies** -- Tasks can declare blockers; [`TaskStore::is_ready`] checks readiness
- **Multi-agent coordination** -- Assignee field for distributing work across sub-agents
- **Persistent storage** -- Git-friendly JSONL files, one per root task tree

# Key types

- [`TasksMcp`] -- The MCP server exposing task tools
- [`Task`] -- A single task with status, assignee, dependencies, and research fields
- [`TaskId`] -- Hierarchical identifier (`at-{hash}` for roots, `at-{hash}.{n}` for subtasks)
- [`TaskStatus`] -- `Pending`, `InProgress`, `Completed`, `Blocked`
- [`TaskStore`] -- JSONL file storage, held in memory in [`TaskId`] order

# Usage

```rust,ignore
use mcp_servers::TasksMcp;
use mcp_utils::server::McpServer;

// Session-scoped (temp dir, auto-cleanup)
let server = McpServer::new(TasksMcp::new());

// Persistent across sessions
let server = McpServer::new(TasksMcp::new_persistent("."));
```
