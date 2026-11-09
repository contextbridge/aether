---
name: review
description: Reviews the current branch, including staged and unstaged changes, and outputs prioritized recommendations to simplify architecture and reduce complexity. Use when the user wants a staff-level review of the current branch without modifying files.
user-invocable: true
agent-invocable: false
---

Review the changes on this branch, including staged/unstaged changes, as a staff+ engineer look for opportunities to simplify, reduce complexity, DRY or make more elegant. Don't modify files but output prioritized recs. Don't be afraid of large refactors that acheive a better architecture.

Start with `git status --short`, `git diff --stat`, and `git diff --cached --stat` (plus `git diff --stat <base>...HEAD` for committed branch changes), then read diffs per file or area (`git diff -- <path>`) as needed rather than dumping the full diff at once.
