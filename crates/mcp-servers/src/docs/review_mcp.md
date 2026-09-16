Review MCP opens agent-authored Markdown in Aether's existing review interface.

It exposes one tool, `review_artifact`. Provide a tagged `source`: `file` with a `path` to an existing regular UTF-8 file, or `content` with Markdown to send directly without creating a file. A top-level `title` optionally labels either source. Relative paths resolve against the agent-side workspace root; absolute paths refer to the agent machine. The server sends the Markdown through MCP elicitation and never creates or modifies an artifact.

The response round returns `approved` for explicit approval without feedback, `feedback` with complete formatted feedback, or `cancelled`/`declined`. Accepted elicitation content requires `decision: approved | feedback`; empty feedback is not approval. The server is stateless and does not reread the file on the response round.
