use crossterm::event::KeyCode;

use super::select_option::SelectOption;
use crate::components::{Component, Event, ViewContext};
use crate::line::Line;
use crate::rendering::frame::Frame;
use crate::style::Style;

/// Single-select from a list of options, rendered as radio buttons.
pub struct RadioSelect {
    pub options: Vec<SelectOption>,
    pub selected: usize,
}

impl RadioSelect {
    pub fn new(options: Vec<SelectOption>, selected: usize) -> Self {
        Self { options, selected }
    }

    pub fn to_json(&self) -> serde_json::Value {
        self.options.get(self.selected).map_or(serde_json::Value::Null, |o| serde_json::Value::String(o.value.clone()))
    }

    fn render_inline(&self, context: &ViewContext) -> Line {
        if let Some(opt) = self.options.get(self.selected) {
            Line::styled(&opt.title, context.theme.info())
        } else {
            Line::default()
        }
    }

    fn render_options(&self, context: &ViewContext) -> Vec<Line> {
        self.options
            .iter()
            .enumerate()
            .map(|(j, opt)| {
                let marker = if j == self.selected { "● " } else { "○ " };
                let style = if j == self.selected { Style::fg(context.theme.primary()) } else { Style::default() };
                Line::with_style(format!("{marker}{}", opt.title), style)
            })
            .collect()
    }
}

impl Component for RadioSelect {
    type Message = ();

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        let Event::Key(key) = event else {
            return None;
        };
        if self.options.is_empty() {
            return None;
        }

        match key.code {
            KeyCode::Up => {
                self.selected = (self.selected + self.options.len() - 1) % self.options.len();
                Some(vec![])
            }
            KeyCode::Down => {
                self.selected = (self.selected + 1) % self.options.len();
                Some(vec![])
            }
            _ => None,
        }
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        Frame::new(self.render_field(context, true))
    }
}

impl RadioSelect {
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

    fn sample_options() -> Vec<SelectOption> {
        vec![
            SelectOption { value: "a".into(), title: "Alpha".into(), description: None },
            SelectOption { value: "b".into(), title: "Beta".into(), description: None },
            SelectOption { value: "c".into(), title: "Gamma".into(), description: None },
        ]
    }

    #[tokio::test]
    async fn down_cycles_forward() {
        let mut rs = RadioSelect::new(sample_options(), 0);
        rs.on_event(&Event::Key(key(KeyCode::Down))).await;
        assert_eq!(rs.selected, 1);
        rs.on_event(&Event::Key(key(KeyCode::Down))).await;
        assert_eq!(rs.selected, 2);
        rs.on_event(&Event::Key(key(KeyCode::Down))).await;
        assert_eq!(rs.selected, 0); // wraps
    }

    #[tokio::test]
    async fn up_cycles_backward() {
        let mut rs = RadioSelect::new(sample_options(), 0);
        rs.on_event(&Event::Key(key(KeyCode::Up))).await;
        assert_eq!(rs.selected, 2); // wraps to end
    }

    #[tokio::test]
    async fn left_right_ignored() {
        let mut rs = RadioSelect::new(sample_options(), 0);
        let outcome = rs.on_event(&Event::Key(key(KeyCode::Right))).await;
        assert!(outcome.is_none());
        assert_eq!(rs.selected, 0);
        let outcome = rs.on_event(&Event::Key(key(KeyCode::Left))).await;
        assert!(outcome.is_none());
        assert_eq!(rs.selected, 0);
    }

    #[test]
    fn to_json_returns_selected_value() {
        let rs = RadioSelect::new(sample_options(), 1);
        assert_eq!(rs.to_json(), serde_json::json!("b"));
    }

    #[test]
    fn to_json_empty_options_returns_null() {
        let rs = RadioSelect::new(vec![], 0);
        assert_eq!(rs.to_json(), serde_json::Value::Null);
    }

    #[tokio::test]
    async fn empty_options_ignores_keys() {
        let mut rs = RadioSelect::new(vec![], 0);
        let outcome = rs.on_event(&Event::Key(key(KeyCode::Right))).await;
        assert!(outcome.is_none());
    }
}
