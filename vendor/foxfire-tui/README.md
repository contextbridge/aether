# foxfire-tui

The terminal UI behind the [`foxfire`](https://crates.io/crates/foxfire) binary: a terminal interface for AI coding agents, built on the Agent Client Protocol (ACP) v2.

Foxfire launches an ACP-compatible agent as a subprocess, streams markdown responses and tool activity, and provides built-in session management, file attachments, plan review, git review, and settings without leaving the terminal.

## Quick start

```bash
cargo install foxfire
foxfire                      # launches the default agent ("aether acp")
foxfire --agent "my-agent"   # launches a custom ACP agent
```

The `--agent` flag accepts any shell command that speaks ACP v2 over stdio.

## Keybindings

| Key | Action |
| --- | --- |
| `Enter` | Send message |
| `Esc` | Cancel the active operation |
| `Ctrl+C` twice | Exit |
| `Tab` | Cycle reasoning effort |
| `Shift+Tab` | Cycle mode/profile |
| `/` | Open command picker |
| `@` | Open file picker |
| `Ctrl+G` | Toggle git review |
| `Ctrl+R` | Search prompt history when supported |

Global bindings can be overridden in Foxfire settings.

## Commands

Type `/` in the composer to open the command picker. Built-in commands include:

| Command | Description |
| --- | --- |
| `/clear` | Clear the conversation and start a new session |
| `/settings` | Open settings and authentication |
| `/resume` | Resume a previous session |

The connected agent may advertise additional commands.

## Settings and themes

UI preferences are stored in `~/.foxfire/settings.json`. Set `FOXFIRE_HOME` to use a different configuration directory. Supported settings include themes, content padding, status-line segments, and global keybindings.

Choose a Clankerdiff built-in theme in `/settings`, or place versioned Clankerdiff JSON theme files in `~/.foxfire/themes/`. A built-ins use `{"source":"builtin","id":"sage"}`; custom uses the JSON filename:

```json
{
  "theme": { "source": "file", "file": "my-theme.json" },
  "keybindings": {
    "toggleGitDiff": "ctrl+d"
  }
}
```

Agent-provided options such as model, reasoning effort, modes, provider login, and MCP server authentication are available through `/settings`.

## Logs

Logs are written to `/tmp/foxfire-logs/foxfire.log.YYYY-MM-DD` by default. Override the directory with:

```bash
foxfire --log-dir ~/logs
```

Use `RUST_LOG` to configure verbosity.

## Library API

Build the API documentation with:

```bash
cargo doc -p foxfire-tui --open
```

The primary public entry points are:

- `run_tui` — connect to an ACP agent and launch Foxfire
- `run_remote_tui` — attach Foxfire to an Aether remote host over an established transport
- `UiSettings` and the `settings` module — persistent UI configuration

The `testing` feature exposes Foxfire's in-memory integration harness for project tests.
