# aether-mcp-utils

[Model Context Protocol](https://modelcontextprotocol.io/) (MCP) building blocks on top of
[rmcp](https://docs.rs/rmcp): connect to a server, host a server, or aggregate many servers
behind one catalog of tools.

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Feature Flags](#feature-flags)
- [License](#license)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Feature Flags

| Feature | Description | Default |
|---------|-------------|---------|
| `client` | `client`, `gateway`, and `config`, including OAuth. Without it, only server hosting remains. | yes |
| `testing` | `testing`: an in-memory `FakeMcpServer` and a scripted elicitation host | no |

## License

MIT
