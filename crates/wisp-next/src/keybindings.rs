use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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

#[derive(Clone, Debug)]
pub struct KeyBinding {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyBinding {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    pub fn matches(&self, event: KeyEvent) -> bool {
        self.code == event.code && self.modifiers == event.modifiers
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_exact_modifier_match() {
        let binding = KeyBinding::new(KeyCode::Enter, KeyModifiers::NONE);
        assert!(!binding.matches(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)));
        assert!(binding.matches(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    }

    #[test]
    fn cycle_mode_matches_shift_tab_reported_with_no_modifiers() {
        // Terminals deliver Shift+Tab as the BackTab keycode with no modifiers, so the
        // default binding must use NONE rather than SHIFT to actually match.
        let kb = Keybindings::default();
        assert!(kb.cycle_mode.matches(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE)));
        assert!(!kb.cycle_mode.matches(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)));
    }
}
