# Tasks MCP Server
Task management tools for deep research agent workflows.

Tasks are organized into trees with:
- Root tasks that can have multiple subtasks
- Dependencies between tasks (a task won't start until its deps complete)
- Assignees for multi-agent coordination

Task IDs:
- Root: `at-{hash}` (e.g., `at-a1b2c3d4`)
- Subtask: `at-{hash}.{n}` (e.g., `at-a1b2c3d4.1`)
