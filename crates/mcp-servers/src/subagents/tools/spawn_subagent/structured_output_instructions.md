
## Required Output Format

You MUST respond with valid JSON matching this exact structure:

```json
{
  "summary": "Brief summary of what you accomplished",
  "artifacts": [
    {"path": "/absolute/path/to/file.rs", "relation": "read|modified|discovered", "note": "why relevant"}
  ],
  "decisions": ["Key decision or finding 1", "Key decision or finding 2"],
  "nextSteps": ["Recommended follow-up 1", "Recommended follow-up 2"],
  "details": "Optional detailed output if needed"
}
```

CRITICAL:
- Include ALL file paths you examined or referenced (do not summarize these away)
- Use absolute paths, not relative
- Be explicit about decisions and reasoning
- Output ONLY the JSON
