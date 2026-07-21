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
    pub open_command_picker: KeyBinding,
    pub open_file_picker: KeyBinding,
    pub toggle_git_diff: KeyBinding,
    pub cycle_reasoning: KeyBinding,
    pub cycle_mode: KeyBinding,
    pub open_prompt_search: KeyBinding,
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
            open_command_picker: KeyBinding::new(KeyCode::Char('/'), KeyModifiers::NONE),
            open_file_picker: KeyBinding::new(KeyCode::Char('@'), KeyModifiers::NONE),
            toggle_git_diff: KeyBinding::new(KeyCode::Char('g'), KeyModifiers::CONTROL),
            cycle_reasoning: KeyBinding::new(KeyCode::Tab, KeyModifiers::NONE),
            cycle_mode: KeyBinding::new(KeyCode::BackTab, KeyModifiers::NONE),
            open_prompt_search: KeyBinding::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
        }
    }
}
