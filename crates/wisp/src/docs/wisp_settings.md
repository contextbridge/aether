Persisted UI preferences for Wisp.

Serialized as JSON to `~/.wisp/settings.json` (or `$WISP_HOME/settings.json`).
Controls theme, content padding, and status-line preferences via [`WispSettings`].
Aether-backed terminal sessions use this same file for UI preferences.

# JSON shape

```json
{
  "theme": {
    "file": "sage.tmTheme"
  },
  "statusLine": {
    "separator": " · ",
    "left": ["cwd", "gitRef"],
    "right": ["mode", { "type": "model", "maxWidth": 32 }, "reasoning", "context"]
  }
}
```

Set `"file"` to `null` or omit it to use the default theme. The filename must be a bare basename pointing to a `.tmTheme` file in `~/.wisp/themes/`.

# See also

- [`load_or_create_settings`](crate::settings::load_or_create_settings) — reads or creates this file
- [`load_theme`](crate::settings::load_theme) — resolves the theme from these settings
- [`StatusLineSettings`](crate::settings::StatusLineSettings) — status-line segment configuration
