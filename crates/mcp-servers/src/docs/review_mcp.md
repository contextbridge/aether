Review MCP opens agent-authored Markdown in Aether's existing review interface.

It exposes one tool, `review_artifact`, with required `path` and `format` (`markdown`) inputs. Relative paths resolve against the agent-side workspace root; absolute paths refer to the agent machine. The server reads a regular UTF-8 file, sends those Markdown bytes through MCP elicitation, and never modifies the artifact.

The retry returns `submitted` with the `approve` or `deny` decision and complete formatted feedback, or returns `cancelled`/`declined`. The server is stateless and does not reread the file on the response round.
