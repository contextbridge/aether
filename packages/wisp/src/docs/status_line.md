The bottom status bar showing workspace and agent state at a glance.


**Built-in segment types:**

| Type            | Description                                                       |
|-----------------|-------------------------------------------------------------------|
| `cwd`           | Current working directory, shortened relative to `$HOME`          |
| `gitRef`        | Current git branch or detached HEAD short SHA                     |
| `agent`         | Active agent name                                                 |
| `mode`          | Current mode/profile (if configured)                              |
| `model`         | Active model name (supports `maxWidth`)                           |
| `reasoning`     | Reasoning effort bar (visual level indicator)                     |
| `context`       | Context window usage bar (current usage against model limit)      |
| `serverHealth`  | Unhealthy MCP server count (hidden when not applicable)           |
| `text`          | Custom static text with an optional semantic style                |

Segments that have no data (e.g. `gitRef` outside a repo, `serverHealth` when
healthy) are silently omitted. 

Segments can be written as string shorthand (e.g. `"cwd"` or `"agent"`) or object form (`{ "type": "cwd" }`). Custom text segments support an optional `style` field with values: `primary`, `secondary`, `muted`, `info`, `success`, `warning`, `error`.

## Sections

Status lines contain a `left` and optional `right` section. If the status line's width exceeds the terminal width, status lines wrap.

# See also

- [`App`](crate::components::app::App) — constructs this view each render cycle
- [`Keybindings`](crate::keybindings::Keybindings) — `Tab` cycles reasoning, `Shift+Tab` cycles mode
- [`StatusLineSettings`](crate::settings::StatusLineSettings) — segment configuration
