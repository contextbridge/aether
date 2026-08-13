Executes a bash command. Each call runs in a fresh shell — no state persists between calls.

**For terminal operations only** (git, npm, docker, cargo, etc.). Use dedicated tools for file operations.

## Usage

```json
{"command": "cargo test", "description": "Run tests"}
{"command": "git status && git diff", "description": "Check git status and diff"}
{"command": "npm run build", "timeout": 300000, "description": "Build with 5min timeout"}
```

- `command` — **required**, the bash command
- `description` — concise description (5-10 words)
- `timeout` — max runtime in ms (max: 600000)
- `run_in_background` — Run a task in the background. You don't need to poll, results are delivered automatically into context when it completes.

## Tips

- Run independent commands in parallel with multiple `bash` calls
- Chain dependent commands with `&&` in a single call
- Use `;` only if you don't care if earlier commands fail
