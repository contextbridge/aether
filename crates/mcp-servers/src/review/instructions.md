# Review MCP Server

Use `review_artifact` to open an agent-authored Markdown file in Aether's review interface and collect explicit approval or contextual feedback. The tool reads but never modifies the artifact.

On `approved`, continue the requested workflow. On `feedback`, address the comments and request review again; empty feedback is not approval. On `cancelled` or `declined`, stop the review loop.
