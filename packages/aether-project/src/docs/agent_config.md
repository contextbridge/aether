A single agent definition. Every agent must be invocable on at least one
surface — set `userInvocable`, `agentInvocable`, or both.

A user-invocable agent with its own prompt and a read-only tool allowlist:

```json
{
  "name": "Review",
  "description": "Reviews diffs and suggests changes",
  "model": "anthropic:claude-sonnet-4-5-20250929",
  "userInvocable": true,
  "prompts": [".aether/REVIEW.md"],
  "tools": { "allow": [{ "readOnly": true }, "plan__*"] }
}
```

A deterministic judge agent that pins sampling via `modelSettings`:

```json
{
  "name": "Judge",
  "description": "Grades responses against a rubric",
  "model": "anthropic:claude-sonnet-4-5-20250929",
  "userInvocable": true,
  "modelSettings": { "temperature": 0, "maxTokens": 1024 }
}
```

A sub-agent (callable by other agents) that pins a Bedrock inference profile:

```json
{
  "name": "Search",
  "description": "Answers questions about the codebase",
  "model": "anthropic.claude-sonnet-4-5-20250929-v1:0",
  "agentInvocable": true,
  "providers": {
    "bedrock": {
      "inferenceProfileArn": "arn:aws:bedrock:us-west-2:000000000000:application-inference-profile/abc"
    }
  }
}
```
