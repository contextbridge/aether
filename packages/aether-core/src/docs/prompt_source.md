Authored description of a prompt source — either a file path string or a typed
text, file, or glob object.

A file path (the most common form):

```json
"AGENTS.md"
```

Inline text:

```json
{ "type": "text", "text": "You are a careful, concise engineer." }
```

A glob that loads every matching file:

```json
{ "type": "glob", "pattern": ".aether/prompts/*.md", "optional": true }
```
