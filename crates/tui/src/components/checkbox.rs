use crossterm::event::KeyCode;

use crate::components::{Component, Event, ViewContext};
use crate::line::Line;
use crate::rendering::frame::Frame;
use crate::style::Style;

/// Boolean toggle rendered as `[x]` / `[ ]`, optionally with an inline label: `[x] Label`.
pub struct Checkbox {
    pub checked: bool,
    label: Option<String>,
}

impl Checkbox {
    pub fn new(checked: bool) -> Self {
        Self { checked, label: None }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::Value::Bool(self.checked)
    }
}

impl Component for Checkbox {
    type Message = ();

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        let Event::Key(key) = event else {
            return None;
        };
        match key.code {
            KeyCode::Char(' ') => {
                self.checked = !self.checked;
                Some(vec![])
            }
            _ => None,
        }
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        Frame::new(self.render_field(context, true))
    }
}

impl Checkbox {
    pub fn render_field(&self, context: &ViewContext, focused: bool) -> Vec<Line> {
        let marker = if self.checked { "[x]" } else { "[ ]" };
        let marker_color = if focused { context.theme.primary() } else { context.theme.text_primary() };
        let mut line = Line::styled(marker, marker_color);
        if let Some(label) = &self.label {
            line.push_with_style(format!(" {label}"), Style::fg(context.theme.text_primary()));
        }
        vec![line]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[tokio::test]
    async fn space_toggles() {
        let mut cb = Checkbox::new(false);
        cb.on_event(&Event::Key(key(KeyCode::Char(' ')))).await;
        assert!(cb.checked);
        cb.on_event(&Event::Key(key(KeyCode::Char(' ')))).await;
        assert!(!cb.checked);
    }

    #[test]
    fn to_json_returns_bool() {
        assert_eq!(Checkbox::new(true).to_json(), serde_json::json!(true));
        assert_eq!(Checkbox::new(false).to_json(), serde_json::json!(false));
    }

    #[tokio::test]
    async fn other_keys_are_ignored() {
        let mut cb = Checkbox::new(false);
        let outcome = cb.on_event(&Event::Key(key(KeyCode::Char('a')))).await;
        assert!(outcome.is_none());
        assert!(!cb.checked);
    }
}
