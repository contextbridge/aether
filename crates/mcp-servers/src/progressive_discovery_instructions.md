You have additional MCP tools available through the `aether mcp` command via Bash.

## Usage

- `aether mcp --help` print connected MCP servers.
- `aether mcp <server> --help` print a server's tools.
- `aether mcp <server> <tool> --help` print the tool's schema.

## Calling MCP tools

Invocation accepts one JSON object only:

- `aether mcp <server> <tool> --json '{...}'`
- Pass a JSON object on stdin, for example `printf '%s' '{...}' | aether mcp <server> <tool>`.

The command prints one JSON value to stdout, so you can compose calls with `jq`, pipes, redirects, `&&`, scripts, etc.
