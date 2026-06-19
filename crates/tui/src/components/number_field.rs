use crossterm::event::KeyCode;

use crate::components::{Component, Event, ViewContext};
use crate::line::Line;
use crate::rendering::frame::Frame;

/// Numeric input field supporting integers or floats.
pub struct NumberField {
    pub value: String,
    pub integer_only: bool,
}

impl NumberField {
    pub fn new(value: String, integer_only: bool) -> Self {
        Self { value, integer_only }
    }

    pub fn to_json(&self) -> serde_json::Value {
        if self.integer_only {
            self.value.parse::<i64>().map_or(serde_json::Value::Null, serde_json::Value::from)
        } else {
            self.value.parse::<f64>().map_or(serde_json::Value::Null, serde_json::Value::from)
        }
    }
}

impl Component for NumberField {
    type Message = ();

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        let Event::Key(key) = event else {
            return None;
        };
        match key.code {
            KeyCode::Char(c) => {
                let accept = c.is_ascii_digit()
                    || (c == '-' && self.value.is_empty())
                    || (c == '.' && !self.integer_only && !self.value.contains('.'));
                if accept {
                    self.value.push(c);
                }
                Some(vec![])
            }
            KeyCode::Backspace => {
                self.value.pop();
                Some(vec![])
            }
            _ => None,
        }
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        Frame::new(self.render_field(context, true))
    }
}

impl NumberField {
    pub fn render_field(&self, context: &ViewContext, focused: bool) -> Vec<Line> {
        let mut line = Line::new(&self.value);
        if focused {
            line.push_styled("▏", context.theme.primary());
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
    async fn integer_accepts_digits_and_leading_minus() {
        let mut field = NumberField::new(String::new(), true);
        field.on_event(&Event::Key(key(KeyCode::Char('-')))).await;
        field.on_event(&Event::Key(key(KeyCode::Char('4')))).await;
        field.on_event(&Event::Key(key(KeyCode::Char('2')))).await;
        assert_eq!(field.value, "-42");
    }

    #[tokio::test]
    async fn integer_rejects_dot() {
        let mut field = NumberField::new("1".to_string(), true);
        field.on_event(&Event::Key(key(KeyCode::Char('.')))).await;
        assert_eq!(field.value, "1");
    }

    #[tokio::test]
    async fn float_accepts_single_dot() {
        let mut field = NumberField::new(String::new(), false);
        field.on_event(&Event::Key(key(KeyCode::Char('3')))).await;
        field.on_event(&Event::Key(key(KeyCode::Char('.')))).await;
        field.on_event(&Event::Key(key(KeyCode::Char('5')))).await;
        assert_eq!(field.value, "3.5");
    }

    #[tokio::test]
    async fn float_rejects_second_dot() {
        let mut field = NumberField::new("1.2".to_string(), false);
        field.on_event(&Event::Key(key(KeyCode::Char('.')))).await;
        assert_eq!(field.value, "1.2");
    }

    #[tokio::test]
    async fn minus_rejected_when_not_first() {
        let mut field = NumberField::new("5".to_string(), true);
        field.on_event(&Event::Key(key(KeyCode::Char('-')))).await;
        assert_eq!(field.value, "5");
    }

    #[test]
    fn to_json_integer() {
        let field = NumberField::new("42".to_string(), true);
        assert_eq!(field.to_json(), serde_json::json!(42));
    }

    #[test]
    fn to_json_float() {
        let field = NumberField::new("3.14".to_string(), false);
        #[allow(clippy::approx_constant)]
        let expected = serde_json::json!(3.14);
        assert_eq!(field.to_json(), expected);
    }

    #[test]
    fn to_json_empty_returns_null() {
        let field = NumberField::new(String::new(), true);
        assert_eq!(field.to_json(), serde_json::Value::Null);
    }

    #[tokio::test]
    async fn backspace_removes_last() {
        let mut field = NumberField::new("12".to_string(), true);
        field.on_event(&Event::Key(key(KeyCode::Backspace))).await;
        assert_eq!(field.value, "1");
    }
}
