A single keyboard shortcut: a key code plus modifier flags.

Use [`matches`](KeyBinding::matches) to test whether an incoming [`KeyEvent`](tui::KeyEvent) corresponds to this binding. The match checks that the key code is equal and that the event's modifiers contain (at minimum) the binding's required modifiers.

# See also

- [`Keybindings`](crate::keybindings::Keybindings) — the full set of application shortcuts
