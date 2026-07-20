# wisp

A terminal interface for AI coding agents, built on the Agent Client Protocol (ACP).

Wisp launches an ACP-compatible agent as a subprocess, streams markdown responses and tool activity, and provides built-in session management, file attachments, plan review, git review, and settings without leaving the terminal.

## Quick start

```bash
cargo install aether-wisp
wisp                       # launches the default agent ("aether acp")
wisp --agent "my-agent"    # launches a custom ACP agent
```

The `--agent` flag accepts any shell command that speaks ACP over stdio.

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

Global bindings can be overridden in Wisp settings.

## Commands

Type `/` in the composer to open the command picker. Built-in commands include:

| Command | Description |
| --- | --- |
| `/clear` | Clear the conversation and start a new session |
| `/settings` | Open settings and authentication |
| `/resume` | Resume a previous session |

The connected agent may advertise additional commands.

## Settings and themes

UI preferences are stored in `~/.wisp/settings.json`. Set `WISP_HOME` to use a different configuration directory. Supported settings include themes, content padding, status-line segments, and global keybindings.

Place TextMate `.tmTheme` files in `~/.wisp/themes/` and select one in settings:

```json
{
  "theme": { "file": "my-theme.tmTheme" },
  "keybindings": {
    "toggleGitDiff": "ctrl+d"
  }
}
```

Agent-provided options such as model, reasoning effort, modes, provider login, and MCP server authentication are available through `/settings`.

## Logs

Logs are written to `/tmp/wisp-logs/wisp.log.YYYY-MM-DD` by default. Override the directory with:

```bash
wisp --log-dir ~/logs
```

Use `RUST_LOG` to configure verbosity.

## Library API

Build the API documentation with:

```bash
cargo doc -p aether-wisp --open
```

The primary public entry points are:

- `run_tui` — connect to an ACP agent and launch Wisp
- `run_with_session` — launch Wisp with an initialized `Session`
- `Session` — an initialized ACP session
- `UiSettings` and the `settings` module — persistent UI configuration

The `testing` feature exposes Wisp's in-memory integration harness for project tests.
