All keyboard shortcuts for the Wisp TUI.

Each field is a [`KeyBinding`](crate::keybindings::KeyBinding) with sensible defaults. Components match incoming key events against these bindings via [`KeyBinding::matches`](crate::keybindings::KeyBinding::matches).

# Default bindings

| Field | Default | Action |
|-------|---------|--------|
| `exit` | `Ctrl+C` | Quit the application |
| `cancel` | `Esc` | Cancel current operation or dismiss modal |
| `cycle_reasoning` | `Tab` | Cycle through reasoning effort levels |
| `cycle_mode` | `Shift+Tab` | Cycle through agent modes/profiles |
| `submit` | `Enter` | Send the current prompt |
| `open_command_picker` | `/` | Open the slash-command picker |
| `open_file_picker` | `@` | Open the file attachment picker |
| `toggle_git_diff` | `Ctrl+G` | Switch between conversation and git diff screens |

# See also

- [`KeyBinding`](crate::keybindings::KeyBinding) — a single key + modifier pair
- [`App`](crate::components::app::App) — uses these bindings for top-level event routing
