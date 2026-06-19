# TasksMcp

Hierarchical task management with dependencies. Tasks are stored as JSONL files for easy version control.

**Flag:** `--dir <path>` (defaults to `.`; creates `.aether-tasks/` inside)

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Tools](#tools)
- [Task IDs](#task-ids)
- [Task Status](#task-status)
- [Research Metadata](#research-metadata)
- [Storage Format](#storage-format)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Tools

| Tool | Description |
|------|-------------|
| `task_create` | Create a task. Optionally set a parent (for subtasks), assignee, and dependency list. |
| `task_get` | Get a task by ID with all metadata. |
| `task_list` | List tasks with filters: assignee, status, tree ID, or `ready_only` (no unresolved dependencies). |
| `task_update` | Update a task's title, description, status, assignee, dependencies, or research metadata. Returns any tasks that become ready when dependencies complete. |

## Task IDs

- Root tasks: `at-a1b2c3d4` (8-character hash)
- Subtasks: `at-a1b2c3d4.1`, `at-a1b2c3d4.2` (dot-notation children)

## Task Status

| Status | Description |
|--------|-------------|
| `Pending` | Not started. |
| `InProgress` | Actively being worked on. |
| `Completed` | Done. Completing a task may unblock dependents. |
| `Blocked` | Waiting on dependencies. |

## Research Metadata

Tasks include optional fields for tracking research context:

| Field | Description |
|-------|-------------|
| `summary` | High-level summary of findings. |
| `decisions` | Key decisions made during the task. |
| `facts` | Discovered facts relevant to the task. |
| `next_steps` | Suggested follow-up actions. |
| `blockers` | Known blockers or open questions. |
| `files_read` | Files consulted during the task. |
| `resources` | External resources referenced. |

## Storage Format

Tasks are stored as JSONL in `.aether-tasks/`. Each root task tree gets its own file (`at-{hash}.jsonl`), making it straightforward to track task state in git.
