---
name: aether-docs
description: Authoritative documentation for Aether itself — its settings, built-in MCP servers, agents, terminal UI, and how to run it. Use whenever the user asks how Aether works, how to configure it, or about its features, commands, or settings.
agent-invocable: true
---

# Aether documentation

When the user asks a question about **Aether**, do the following:

1. Fetch `https://aether-agent.io/llms-small.txt` with the `web_fetch` tool. Or donwload the file (e.g. using `curl`).
2. Answer from the fetched content, citing the relevant section when useful.
3. If more detail is required, fetch `https://aether-agent.io/llms-full.txt` (note this is context intensive).

If a fetch fails (e.g. no network), say so plainly rather than guessing.
