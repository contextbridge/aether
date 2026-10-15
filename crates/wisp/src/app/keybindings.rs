//! Global command bindings and the application's modifier policy.
//!
//! Policy: global commands (everything in [`Keybindings`]) are user
//! configurable through `keybindings` in the settings file and match their
//! modifiers exactly, so a binding on Ctrl+C never also fires on
//! Ctrl+Shift+C. Feature-local editing and navigation keys are fixed: they
//! are matched next to the behavior they trigger and test only the modifiers
//! they care about, which keeps them tolerant of terminals that add stray
//! modifier flags.

use crate::settings::{KeyAction, UiSettings};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;
use std::str::FromStr;

/// The user's overrides; every other action uses its default binding.
#[derive(Clone, Debug)]
pub struct Keybindings(BTreeMap<KeyAction, KeyBinding>);

impl Keybindings {
    /// Parses the configured overrides. An entry that does not parse keeps its
    /// default and logs a warning rather than dropping the command.
    pub fn from_settings(settings: &UiSettings) -> Self {
        let mut overrides = BTreeMap::new();
        for (&action, text) in &settings.keybindings {
            match text.parse() {
                Ok(parsed) => {
                    overrides.insert(action, parsed);
                }
                Err(KeyBindingParseError(input)) => {
                    tracing::warn!("ignoring unparseable keybinding {action:?} = {input:?}");
                }
            }
        }
        Self(overrides)
    }

    /// Whether `key` triggers `action`, per the module's modifier policy.
    pub fn matches(&self, action: KeyAction, key: KeyEvent) -> bool {
        self.0.get(&action).copied().unwrap_or_else(|| action.default_binding()).matches(key)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyBinding {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyBinding {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    /// Exact-match semantics: the key and the full modifier set must both
    /// match, per the module's modifier policy.
    pub fn matches(&self, event: KeyEvent) -> bool {
        self.code == event.code && self.modifiers == event.modifiers
    }
}

/// The rejected input, for the warning that keeps the default binding.
#[derive(Debug, PartialEq, Eq)]
pub struct KeyBindingParseError(pub String);

impl FromStr for KeyBinding {
    type Err = KeyBindingParseError;

    /// Parses `"ctrl+g"`, `"shift+backtab"`, `"esc"`, `"f5"`, `"@"`, ...:
    /// any number of modifier tokens followed by one key token, joined by `+`.
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let error = || KeyBindingParseError(input.to_string());
        let mut modifiers = KeyModifiers::NONE;
        let tokens: Vec<&str> = input.split('+').map(str::trim).collect();
        let (&key, modifier_tokens) = tokens.split_last().ok_or_else(error)?;
        for token in modifier_tokens {
            modifiers |= match token.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => KeyModifiers::CONTROL,
                "alt" | "option" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                "super" | "cmd" => KeyModifiers::SUPER,
                "meta" => KeyModifiers::META,
                "hyper" => KeyModifiers::HYPER,
                _ => return Err(error()),
            };
        }
        let code = parse_key_code(key).ok_or_else(error)?;
        Ok(Self::new(code, modifiers))
    }
}

impl KeyAction {
    fn default_binding(self) -> KeyBinding {
        let (code, modifiers) = match self {
            Self::Exit => (KeyCode::Char('c'), KeyModifiers::CONTROL),
            Self::Cancel => (KeyCode::Esc, KeyModifiers::NONE),
            Self::Submit => (KeyCode::Enter, KeyModifiers::NONE),
            Self::OpenCommandPicker => (KeyCode::Char('/'), KeyModifiers::NONE),
            Self::OpenFilePicker => (KeyCode::Char('@'), KeyModifiers::NONE),
            Self::ToggleGitDiff => (KeyCode::Char('g'), KeyModifiers::CONTROL),
            Self::CycleReasoning => (KeyCode::Tab, KeyModifiers::NONE),
            Self::CycleMode => (KeyCode::BackTab, KeyModifiers::SHIFT),
            Self::OpenPromptSearch => (KeyCode::Char('r'), KeyModifiers::CONTROL),
        };
        KeyBinding::new(code, modifiers)
    }
}

fn parse_key_code(token: &str) -> Option<KeyCode> {
    let mut chars = token.chars();
    if let (Some(character), None) = (chars.next(), chars.next()) {
        return Some(KeyCode::Char(character));
    }
    Some(match token.to_ascii_lowercase().as_str() {
        "esc" | "escape" => KeyCode::Esc,
        "enter" | "return" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "space" => KeyCode::Char(' '),
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "backspace" => KeyCode::Backspace,
        "delete" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        function if function.starts_with('f') => KeyCode::F(function[1..].parse().ok()?),
        _ => return None,
    })
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
    fn parses_modifier_chords_named_keys_and_characters() {
        assert_eq!("ctrl+g".parse(), Ok(KeyBinding::new(KeyCode::Char('g'), KeyModifiers::CONTROL)));
        assert_eq!(
            "Ctrl+Shift+p".parse(),
            Ok(KeyBinding::new(KeyCode::Char('p'), KeyModifiers::CONTROL | KeyModifiers::SHIFT))
        );
        assert_eq!("shift+backtab".parse(), Ok(KeyBinding::new(KeyCode::BackTab, KeyModifiers::SHIFT)));
        assert_eq!("esc".parse(), Ok(KeyBinding::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert_eq!("f5".parse(), Ok(KeyBinding::new(KeyCode::F(5), KeyModifiers::NONE)));
        assert_eq!("@".parse(), Ok(KeyBinding::new(KeyCode::Char('@'), KeyModifiers::NONE)));
        assert!("ctrl+".parse::<KeyBinding>().is_err());
        assert!("bogus+x".parse::<KeyBinding>().is_err());
        assert!("notakey".parse::<KeyBinding>().is_err());
    }

    #[test]
    fn settings_override_defaults_and_ignore_invalid_entries() {
        let settings = UiSettings {
            keybindings: BTreeMap::from([
                (KeyAction::ToggleGitDiff, "ctrl+d".to_string()),
                (KeyAction::Submit, "not a key at all".to_string()),
            ]),
            ..UiSettings::default()
        };
        let bindings = Keybindings::from_settings(&settings);
        assert!(bindings.matches(KeyAction::ToggleGitDiff, KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)));
        assert!(!bindings.matches(KeyAction::ToggleGitDiff, KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)));
        assert!(bindings.matches(KeyAction::Submit, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    }
}
