# Edit Plan

Edits an existing markdown implementation plan in Plan mode. Pass the same `planName` used with `write_plan`, plus an `edits` array applied atomically.

Each edit replaces an exact `oldString` with `newString` (optional `replaceAll`). All edits must apply or none are written, so a multi-point revision lands in a single call. Edits must target non-overlapping regions.

```json
{"planName": "auth-refactor", "edits": [
  {"oldString": "## Old approach\n\nUse cookies.", "newString": "## Revised approach\n\nUse JWT sessions."},
  {"oldString": "Phase 2", "newString": "Phase 3", "replaceAll": true}
]}
```
