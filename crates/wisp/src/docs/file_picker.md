Fuzzy-searchable file picker for `@`-mentions.

Opened by typing `@` in the prompt. Indexes up to 50,000 files from the working directory (respecting `.gitignore` rules) and presents them in a [`Combobox`](tui::Combobox) for fuzzy matching. Selected files are inserted as mentions in the prompt and attached to the next message.

# File discovery

Uses the [`ignore`] crate's `WalkBuilder` to traverse the directory tree, honoring `.gitignore`, global git ignores, and git exclude rules. Hidden files are included but common non-source directories (`node_modules`, `target`, dotfiles) are excluded.

# See also

- [`FileMatch`] — a single file entry with path and display name
- [`CommandPicker`](crate::components::command_picker::CommandPicker) — the `/`-triggered command picker
