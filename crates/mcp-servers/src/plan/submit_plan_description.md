# Submit Plan 

Submits a plan for user approval and/or feedback when in Plan mode.

Pass the `planName` previously used with `write_plan` or `edit_plan`. The server reads the corresponding `<planName>-plan.md` file from the configured plans directory and presents it for review.

Always call this tool when: 

1. You're in plan mode
2. You've written or updated a plan and are ready for the user to review it

Never call this tool when you're not in Plan mode.
