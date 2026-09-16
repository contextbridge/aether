---
name: plan
description: Research, write, and submit an implementation plan for review.
user-invocable: true
---

Create an implementation plan for:

$ARGUMENTS

Research the codebase before drafting. Write the plan to `docs/aether/plans/<name>-plan.md` using `coding__write_file` and revise it with `coding__edit_file`. Do not implement source changes before the user approves the plan.

Read the artifact before editing it. Call `review__review_artifact` with the plan path and `format: markdown`.

- `feedback`: address every contextual comment, update the plan, and request review again. Empty feedback is not approval.
- `approved`: continue the requested workflow without asking for approval again. For planning-only requests, report the plan path and stop; for plan-then-build requests, proceed to implementation.
- `cancelled` or `declined`: stop the review loop without implementing source changes.
