Spawns sub-agents in parallel to perform concurrent tasks.

## Usage

```json
{
  "tasks": [
    {"agentName": "codebase-explorer", "prompt": "Find all API endpoints"},
    {"agentName": "rust-code-monkey", "prompt": "Write tests for auth module"}
  ],
  "runInBackground": false
}
```

- `tasks` — **required**, array of task objects
  - `agentName` — agent name from project `.aether/settings.json` (`agents[].name`) with `agentInvocable: true`
  - `prompt` — task for the agent to perform
- `runInBackground` — optional, defaults to `false`. When `true`, subagents run in the background, so you can continue working -- use when you don't want to wait for the sub-agents to finish; results are automatically injected into context when the sub-agents complete.
