Use this tool to trigger a UI for the user to review a markdown file on the filesystem. Their line-level feedback will be returned to you in the response.

## Usage

```json
{"path": "docs/aether/plans/feature.md", "format": "markdown"}
```

- `path` — **required**, path to an existing file. Relative paths resolve against the configured workspace root.
- `format` — **required**, artifact format. Currently only `markdown` is supported.

**Returns:** `approved` for explicit approval without feedback, `feedback` with the user's complete formatted `feedback`, or `cancelled`/`declined`. Empty feedback is not approval.


## Tips

Call this tool when:

- You write a plan file to the filesystem and want the user to give you feedback on the plan.
- You want feedback on multiple high-level options that could impact the trajectory of your task. For example, you might be in the middle of creating a plan and come up with a few high level options for the user to decide on (e.g. Postgres vs MySQL vs `DynamoDB`). In this case, write a simple markdown file to the filesystem (e.g. a tmp dir) that outlines the options (if discussing code, use code fences to render high level interfaces/schemas etc), then call this tool to get feedback from the user.


On `feedback`, address every contextual comment and re-call the tool. On `approved`, continue the user's requested workflow without asking for approval again. On `cancelled` or `declined`, stop the review loop. Approval does not expand the task: a planning-only request ends with the plan; a plan-then-build request can proceed to implementation.
