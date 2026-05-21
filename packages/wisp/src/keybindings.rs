use tui::{KeyCode, KeyEvent, KeyModifiers};

#[doc = include_str!("docs/key_binding.md")]
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
        self.code == event.code && event.modifiers.contains(self.modifiers)
    }

    pub fn char(&self) -> Option<char> {
        match self.code {
            KeyCode::Char(c) => Some(c),
            _ => None,
        }
    }
}

#[doc = include_str!("docs/keybindings.md")]
#[derive(Clone, Debug)]
pub struct Keybindings {
    pub exit: KeyBinding,
    pub cancel: KeyBinding,
    pub cycle_reasoning: KeyBinding,
    pub cycle_mode: KeyBinding,
    pub submit: KeyBinding,
    pub open_command_picker: KeyBinding,
    pub open_file_picker: KeyBinding,
    pub toggle_git_diff: KeyBinding,
    pub open_prompt_search: KeyBinding,
}

impl Default for Keybindings {
    fn default() -> Self {
        Self {
            exit: KeyBinding::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            cancel: KeyBinding::new(KeyCode::Esc, KeyModifiers::NONE),
            cycle_reasoning: KeyBinding::new(KeyCode::Tab, KeyModifiers::NONE),
            cycle_mode: KeyBinding::new(KeyCode::BackTab, KeyModifiers::NONE),
            submit: KeyBinding::new(KeyCode::Enter, KeyModifiers::NONE),
            open_command_picker: KeyBinding::new(KeyCode::Char('/'), KeyModifiers::NONE),
            open_file_picker: KeyBinding::new(KeyCode::Char('@'), KeyModifiers::NONE),
            toggle_git_diff: KeyBinding::new(KeyCode::Char('g'), KeyModifiers::CONTROL),
            open_prompt_search: KeyBinding::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn matches_simple_key() {
        let binding = KeyBinding::new(KeyCode::Tab, KeyModifiers::NONE);
        assert!(binding.matches(key(KeyCode::Tab)));
        assert!(!binding.matches(key(KeyCode::Enter)));
    }

    #[test]
    fn matches_key_with_modifier() {
        let binding = KeyBinding::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(binding.matches(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        assert!(!binding.matches(key(KeyCode::Char('c'))));
    }

    #[test]
    fn matches_ignores_extra_modifiers() {
        let binding = KeyBinding::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let event = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert!(binding.matches(event));
    }

    #[test]
    fn char_returns_char_for_char_key() {
        let binding = KeyBinding::new(KeyCode::Char('/'), KeyModifiers::NONE);
        assert_eq!(binding.char(), Some('/'));
    }

    #[test]
    fn char_returns_none_for_non_char_key() {
        let binding = KeyBinding::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(binding.char(), None);
    }

    #[test]
    fn default_keybindings_have_expected_values() {
        let kb = Keybindings::default();
        assert_eq!(kb.exit.code, KeyCode::Char('c'));
        assert_eq!(kb.exit.modifiers, KeyModifiers::CONTROL);
        assert_eq!(kb.cancel.code, KeyCode::Esc);
        assert_eq!(kb.cycle_reasoning.code, KeyCode::Tab);
        assert_eq!(kb.cycle_mode.code, KeyCode::BackTab);
        assert_eq!(kb.submit.code, KeyCode::Enter);
        assert_eq!(kb.open_command_picker.code, KeyCode::Char('/'));
        assert_eq!(kb.open_file_picker.code, KeyCode::Char('@'));
        assert_eq!(kb.toggle_git_diff.code, KeyCode::Char('g'));
        assert_eq!(kb.toggle_git_diff.modifiers, KeyModifiers::CONTROL);
        assert_eq!(kb.open_prompt_search.code, KeyCode::Char('r'));
        assert_eq!(kb.open_prompt_search.modifiers, KeyModifiers::CONTROL);
    }
}
