use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Debug)]
pub struct KeyBinding {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

#[derive(Clone, Debug)]
pub struct Keybindings {
    pub exit: KeyBinding,
    pub cancel: KeyBinding,
    pub submit: KeyBinding,
}

impl KeyBinding {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    pub fn matches(&self, event: KeyEvent) -> bool {
        self.code == event.code && event.modifiers.contains(self.modifiers)
    }
}

impl Default for Keybindings {
    fn default() -> Self {
        Self {
            exit: KeyBinding::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            cancel: KeyBinding::new(KeyCode::Esc, KeyModifiers::NONE),
            submit: KeyBinding::new(KeyCode::Enter, KeyModifiers::NONE),
        }
    }
}
