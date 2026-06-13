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
