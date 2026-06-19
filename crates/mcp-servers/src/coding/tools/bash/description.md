Executes bash commands in a persistent shell session.

**For terminal operations only** (git, npm, docker, cargo, etc.). Use dedicated tools for file operations.

## Usage

```json
{"command": "cargo test", "description": "Run tests"}
{"command": "git status && git diff", "description": "Check git status and diff"}
{"command": "npm run build", "timeout": 300000, "description": "Build with 5min timeout"}
```

- `command` — **required**, the bash command
- `description` — concise description (5-10 words)
- `timeout` — max runtime in ms (default: 120000, max: 600000)
- `run_in_background` — run async, check output with `read_background_bash`

## Tips

- Run independent commands in parallel with multiple `bash` calls
- Chain dependent commands with `&&` in a single call
- Use `;` only if you don't care if earlier commands fail
