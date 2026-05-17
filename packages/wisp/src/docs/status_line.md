The bottom status bar showing workspace and agent state at a glance.

Renders a single line with current workspace context on the left and session indicators on the right:

**Left side:**
- Current working directory, shortened relative to the user's home directory when possible
- Current git branch or detached HEAD short SHA when the working directory is in a git repo/worktree

**Right side:**
- Agent name
- Current mode/profile (if configured)
- Active model name
- Reasoning effort bar (visual level indicator)
- Context window usage bar (shows current context usage against the model limit)
- Unhealthy MCP server count (when not waiting for a response)

# See also

- [`App`](crate::components::app::App) — constructs this view each render cycle
- [`Keybindings`](crate::keybindings::Keybindings) — `Tab` cycles reasoning, `Shift+Tab` cycles mode
