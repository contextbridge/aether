use crossterm::event::KeyCode;

use super::select_option::SelectOption;
use crate::components::{Component, Event, ViewContext};
use crate::line::Line;
use crate::rendering::frame::Frame;
use crate::style::Style;

/// Multi-select from a list of options, rendered as checkboxes with a cursor.
pub struct MultiSelect {
    pub options: Vec<SelectOption>,
    pub selected: Vec<bool>,
    pub cursor: usize,
}

impl MultiSelect {
    pub fn new(options: Vec<SelectOption>, selected: Vec<bool>) -> Self {
        Self { cursor: 0, options, selected }
    }

    pub fn to_json(&self) -> serde_json::Value {
        let values: Vec<serde_json::Value> = self
            .options
            .iter()
            .zip(self.selected.iter())
            .filter(|&(_, &s)| s)
            .map(|(o, _)| serde_json::Value::String(o.value.clone()))
            .collect();
        serde_json::Value::Array(values)
    }

    fn render_inline(&self, context: &ViewContext) -> Line {
        let chosen: Vec<&str> =
            self.options.iter().zip(self.selected.iter()).filter(|&(_, &s)| s).map(|(o, _)| o.title.as_str()).collect();

        if chosen.is_empty() {
            Line::styled("(none)", context.theme.muted())
        } else {
            Line::styled(chosen.join(", "), context.theme.info())
        }
    }

    fn render_options(&self, context: &ViewContext) -> Vec<Line> {
        self.options
            .iter()
            .enumerate()
            .map(|(j, opt)| {
                let marker = if self.selected[j] { "[x] " } else { "[ ] " };
                let is_cursor = j == self.cursor;
                let style = if is_cursor {
                    Style::fg(context.theme.primary()).bold()
                } else if self.selected[j] {
                    Style::fg(context.theme.primary())
                } else {
                    Style::default()
                };
                let desc = opt.description.as_deref().map(|d| format!(" - {d}")).unwrap_or_default();
                Line::with_style(format!("{marker}{}{desc}", opt.title), style)
            })
            .collect()
    }
}

impl Component for MultiSelect {
    type Message = ();

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        let Event::Key(key) = event else {
            return None;
        };
        if self.options.is_empty() {
            return None;
        }

        match key.code {
            KeyCode::Char(' ') => {
                self.selected[self.cursor] = !self.selected[self.cursor];
                Some(vec![])
            }
            KeyCode::Up => {
                self.cursor = (self.cursor + self.options.len() - 1) % self.options.len();
                Some(vec![])
            }
            KeyCode::Down => {
                self.cursor = (self.cursor + 1) % self.options.len();
                Some(vec![])
            }
            _ => None,
        }
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        Frame::new(self.render_field(context, true))
    }
}

impl MultiSelect {
    pub fn render_field(&self, context: &ViewContext, focused: bool) -> Vec<Line> {
        if focused { self.render_options(context) } else { vec![self.render_inline(context)] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn sample() -> MultiSelect {
        MultiSelect::new(
            vec![
                SelectOption { value: "a".into(), title: "Alpha".into(), description: None },
                SelectOption { value: "b".into(), title: "Beta".into(), description: None },
                SelectOption { value: "c".into(), title: "Gamma".into(), description: None },
            ],
            vec![false, false, false],
        )
    }

    #[tokio::test]
    async fn space_toggles_at_cursor() {
        let mut ms = sample();
        ms.on_event(&Event::Key(key(KeyCode::Char(' ')))).await;
        assert!(ms.selected[0]);
        ms.on_event(&Event::Key(key(KeyCode::Char(' ')))).await;
        assert!(!ms.selected[0]);
    }

    #[tokio::test]
    async fn cursor_moves_with_arrows() {
        let mut ms = sample();
        ms.on_event(&Event::Key(key(KeyCode::Down))).await;
        assert_eq!(ms.cursor, 1);
        ms.on_event(&Event::Key(key(KeyCode::Char(' ')))).await;
        assert!(ms.selected[1]);
    }

    #[test]
    fn to_json_returns_selected_values() {
        let mut ms = sample();
        ms.selected[0] = true;
        ms.selected[2] = true;
        assert_eq!(ms.to_json(), serde_json::json!(["a", "c"]));
    }

    #[test]
    fn to_json_empty_selection() {
        let ms = sample();
        assert_eq!(ms.to_json(), serde_json::json!([]));
    }

    #[tokio::test]
    async fn cursor_wraps() {
        let mut ms = sample();
        ms.on_event(&Event::Key(key(KeyCode::Up))).await;
        assert_eq!(ms.cursor, 2); // wraps to end
    }

    #[tokio::test]
    async fn left_right_ignored() {
        let mut ms = sample();
        let outcome = ms.on_event(&Event::Key(key(KeyCode::Right))).await;
        assert!(outcome.is_none());
        assert_eq!(ms.cursor, 0);
        let outcome = ms.on_event(&Event::Key(key(KeyCode::Left))).await;
        assert!(outcome.is_none());
        assert_eq!(ms.cursor, 0);
    }
}
