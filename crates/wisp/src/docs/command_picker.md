Fuzzy-searchable picker for slash commands.

Opened by typing `/` in the prompt. Wraps the [`Combobox`](tui::Combobox) widget from the `tui` crate, populated with [`CommandEntry`] items that include both built-in commands (`/clear`, `/settings`, `/resume`) and agent-provided commands.

# Rendering

Each entry shows `/<name>` left-aligned with the description to the right. The selected row is highlighted. An optional `[hint]` suffix appears for commands that accept arguments.

# See also

- [`CommandEntry`] — a single command with name, description, and metadata
- [`Combobox`](tui::Combobox) — the underlying fuzzy picker widget
- [`FilePicker`](crate::components::file_picker::FilePicker) — the `@`-triggered file picker
