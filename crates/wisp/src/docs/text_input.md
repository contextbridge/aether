Single-line text input with cursor navigation and file mention support.

Wraps a [`TextField`](tui::TextField) and adds Wisp-specific behavior: `Enter` submits the prompt, `/` opens the [`CommandPicker`](crate::components::command_picker::CommandPicker), and `@` opens the [`FilePicker`](crate::components::file_picker::FilePicker). Selected file mentions are tracked alongside the text buffer and attached to the outgoing prompt.

# Messages

- **`Submit`** — the user pressed Enter.
- **`OpenCommandPicker`** — `/` was typed (the picker replaces text input).
- **`OpenFilePicker`** — `@` was typed.

# See also

- `PromptComposer` — the parent that composes this input with pickers and attachments
- [`SelectedFileMention`] — metadata for an `@`-mentioned file
