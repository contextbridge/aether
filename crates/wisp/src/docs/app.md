Main application component that orchestrates the entire Wisp TUI.

`App` owns two screens — a `ConversationScreen` for chat and a `ScreenRouter` for navigating to the git diff viewer — plus an optional `SettingsOverlay`.

# Event routing

The parent event loop in [`run_with_state`](crate::run_with_state) feeds three event sources into `App`:

- **Terminal events** — keystrokes and resizes, dispatched through [`Component::on_event`](tui::Component::on_event).
- **ACP events** — streamed text chunks, tool calls, plans, and session lifecycle updates, dispatched through [`on_acp_event`](App::on_acp_event).
- **Ticks** — 100 ms heartbeats for animations (spinners, progress bars), gated by [`wants_tick`](App::wants_tick).

# Screen management

`ScreenRouter` tracks whether the user is viewing the conversation or the git diff. `Ctrl+G` toggles between them. The settings overlay floats above whichever screen is active.

# See also

- `ConversationScreen` — the main chat UI
- [`RuntimeState`](crate::runtime_state::RuntimeState) — ACP session bootstrap that produces the inputs for `App::new`
- [`Keybindings`](crate::keybindings::Keybindings) — all keyboard shortcuts
