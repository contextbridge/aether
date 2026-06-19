Holds everything needed to start the TUI after the ACP handshake completes.

[`RuntimeState::new`](RuntimeState::new) spawns the agent as a subprocess, sends `initialize` and `newSession` requests over ACP, loads the user's theme, and packages the results into this struct. The caller ([`run_tui`](crate::run_tui) or [`run_with_state`](crate::run_with_state)) then destructures it to build [`App`](crate::components::app::App) and the renderer.

# Fields

- **`session_id`** — the ACP session identifier, used for resumption.
- **`agent_name`** — human-readable agent name returned by `initialize`.
- **`prompt_capabilities`** — what the agent supports (slash commands, file mentions, etc.).
- **`config_options`** — agent-advertised settings (model, reasoning effort, mode).
- **`auth_methods`** — provider login methods the agent requires.
- **`theme`** — resolved [`Theme`](tui::Theme) from the user's Wisp settings.
- **`event_rx`** — channel receiver for streamed [`AcpEvent`](acp_utils::client::AcpEvent)s.
- **`prompt_handle`** — handle for sending user prompts back to the agent.
- **`working_dir`** — the working directory passed to the agent session.
- **`workspace_status`** — pre-resolved display metadata for the status line's workspace segment.

# See also

- [`run_tui`](crate::run_tui) — the high-level entry point that creates a `RuntimeState` and runs the TUI
- [`App`](crate::components::app::App) — consumes these fields to build the application
