Allow specific tools by name:

```json
{ "allow": ["read_file", "grep", "find"] }
```

Allow tools annotated as read-only by their MCP server:

```json
{ "allow": [{ "readOnly": true }] }
```

Combine tool matchers with exact or trailing-`*` name patterns:

```json
{ "allow": [{ "readOnly": true }, "plan__*"], "deny": ["coding__web_*"] }
```

Deny always wins over allow:

```json
{ "allow": [{ "readOnly": true }], "deny": [{ "openWorld": true }] }
```

Annotation matcher fields are `readOnly`, `destructive`, `idempotent`, and `openWorld`. Missing tool annotations or missing annotation fields do not receive default boolean values and do not match.

MCP name matchers are evaluated against the canonical namespaced name
(`server__tool`), not the server-local name. The decision is stored in the
per-agent `ToolCatalog` and applies consistently to model-visible definitions,
deferred discovery, instructions, and both model-visible and deferred execution routes.
