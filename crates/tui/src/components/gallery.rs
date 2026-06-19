use crossterm::event::KeyCode;

use super::component::{Component, Event};
use super::panel::Panel;
use super::split_panel::{Either, SplitLayout, SplitPanel};
use super::wrap_selection;
use crate::line::Line;
use crate::rendering::frame::{Cursor, Frame};
use crate::rendering::render_context::ViewContext;
use crate::style::Style;

pub enum GalleryMessage {
    Quit,
}

pub struct Gallery<T: Component> {
    split: SplitPanel<GallerySidebar, GalleryPreview<T>>,
}

impl<T: Component> Gallery<T> {
    pub fn new(entries: Vec<(String, T)>) -> Self {
        let names: Vec<String> = entries.iter().map(|(n, _)| n.clone()).collect();
        let longest = names.iter().map(String::len).max().unwrap_or(0);
        let layout = SplitLayout::fixed((longest + 6).clamp(20, 30));
        let sidebar = GallerySidebar { names, selected: 0, focused: true };
        let preview = GalleryPreview { entries, active: 0, focused: false };

        Self { split: SplitPanel::new(sidebar, preview, layout).with_separator("│", Style::default()) }
    }
}

impl<T: Component> Component for Gallery<T> {
    type Message = GalleryMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        if let Event::Tick = event {
            let _ = self.split.right_mut().on_event(event).await;
            return Some(vec![]);
        }

        if let Event::Key(key) = event
            && key.code == KeyCode::Esc
        {
            return Some(vec![GalleryMessage::Quit]);
        }

        match self.split.on_event(event).await {
            Some(msgs) => {
                for msg in &msgs {
                    if let Either::Left(GallerySidebarMessage::Selected(idx)) = msg {
                        self.split.right_mut().active = *idx;
                    }
                }
                Some(vec![])
            }
            None => None,
        }
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        if self.split.left().names.is_empty() {
            return Frame::new(vec![Line::new("No stories")]);
        }

        let left_focused = self.split.is_left_focused();
        self.split.left_mut().focused = left_focused;
        self.split.right_mut().focused = !left_focused;
        self.split.set_separator_style(Style::fg(ctx.theme.muted()));

        self.split.render(ctx)
    }
}

enum GallerySidebarMessage {
    Selected(usize),
}

struct GallerySidebar {
    names: Vec<String>,
    selected: usize,
    focused: bool,
}

impl Component for GallerySidebar {
    type Message = GallerySidebarMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        if let Event::Key(key) = event {
            match key.code {
                KeyCode::Up => {
                    wrap_selection(&mut self.selected, self.names.len(), -1);
                    Some(vec![GallerySidebarMessage::Selected(self.selected)])
                }
                KeyCode::Down => {
                    wrap_selection(&mut self.selected, self.names.len(), 1);
                    Some(vec![GallerySidebarMessage::Selected(self.selected)])
                }
                _ => Some(vec![]),
            }
        } else {
            None
        }
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let width = ctx.size.width as usize;
        let height = ctx.size.height as usize;
        let mut lines = Vec::with_capacity(height);

        lines.push(Line::with_style(" Gallery", Style::fg(ctx.theme.accent()).bold()));
        lines.push(Line::default());

        for (i, name) in self.names.iter().enumerate() {
            let is_selected = i == self.selected;
            let indicator = if is_selected { ">" } else { " " };
            let style = if is_selected && self.focused {
                ctx.theme.selected_row_style()
            } else if is_selected {
                Style::fg(ctx.theme.text_primary()).bold()
            } else {
                Style::fg(ctx.theme.text_secondary())
            };
            let mut line = Line::with_style(format!(" {indicator} {name}"), style);
            line.extend_bg_to_width(width);
            lines.push(line);
        }

        while lines.len() < height {
            lines.push(Line::default());
        }
        lines.truncate(height);

        Frame::new(lines)
    }
}

struct GalleryPreview<T: Component> {
    entries: Vec<(String, T)>,
    active: usize,
    focused: bool,
}

impl<T: Component> Component for GalleryPreview<T> {
    type Message = ();

    async fn on_event(&mut self, event: &Event) -> Option<Vec<()>> {
        if let Some((_, component)) = self.entries.get_mut(self.active) {
            let _ = component.on_event(event).await;
        }
        match event {
            Event::Key(_) | Event::Tick => Some(vec![]),
            _ => None,
        }
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let (name, component) = &mut self.entries[self.active];
        let border_color = if self.focused { ctx.theme.accent() } else { ctx.theme.muted() };

        let inner_width = Panel::inner_width(ctx.size.width);
        let inner_ctx = ctx.with_size((inner_width, ctx.size.height.saturating_sub(4)));
        let frame = component.render(&inner_ctx);
        let (content_lines, cursor) = frame.into_parts();

        let footer = if self.focused { "[Shift+Tab] sidebar  [Esc] quit" } else { "[Tab] preview  [Esc] quit" };
        let mut panel = Panel::new(border_color).title(format!(" {name} ")).footer(footer);
        panel.push(content_lines);

        let panel_cursor = if self.focused && cursor.is_visible {
            Cursor::visible(cursor.row + 2, cursor.col + 2)
        } else {
            Cursor::hidden()
        };

        panel.render(ctx).with_cursor(panel_cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rendering::line::Line;

    struct DummyComponent {
        label: String,
    }

    impl Component for DummyComponent {
        type Message = ();

        async fn on_event(&mut self, _event: &Event) -> Option<Vec<()>> {
            None
        }

        fn render(&mut self, _ctx: &ViewContext) -> Frame {
            Frame::new(vec![Line::new(&self.label)])
        }
    }

    fn dummy(name: &str, label: &str) -> (String, DummyComponent) {
        (name.into(), DummyComponent { label: label.into() })
    }

    #[test]
    fn empty_gallery_renders_placeholder() {
        let mut gallery: Gallery<DummyComponent> = Gallery::new(vec![]);
        let ctx = ViewContext::new((80, 24));
        let frame = gallery.render(&ctx);
        assert_eq!(frame.lines()[0].plain_text(), "No stories");
    }

    #[test]
    fn sidebar_shows_all_entry_names() {
        let mut gallery = Gallery::new(vec![dummy("Alpha", "a"), dummy("Beta", "b")]);
        let ctx = ViewContext::new((80, 24));
        let frame = gallery.render(&ctx);
        let text: String = frame.lines().iter().map(Line::plain_text).collect::<Vec<_>>().join("\n");
        assert!(text.contains("Alpha"), "should contain Alpha: {text}");
        assert!(text.contains("Beta"), "should contain Beta: {text}");
    }

    #[test]
    fn selected_entry_has_indicator() {
        let mut gallery = Gallery::new(vec![dummy("Alpha", "a"), dummy("Beta", "b")]);
        let ctx = ViewContext::new((80, 24));
        let frame = gallery.render(&ctx);
        let all_text: Vec<String> = frame.lines().iter().map(Line::plain_text).collect();
        assert!(all_text.iter().any(|l| l.contains("> Alpha")), "should have > Alpha indicator: {all_text:?}");
    }

    #[tokio::test]
    async fn down_arrow_changes_selection() {
        let mut gallery = Gallery::new(vec![dummy("Alpha", "a"), dummy("Beta", "b")]);
        assert_eq!(gallery.split.left().selected, 0);

        let down = Event::Key(crossterm::event::KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE));
        gallery.on_event(&down).await;
        assert_eq!(gallery.split.left().selected, 1);
    }

    #[tokio::test]
    async fn esc_emits_quit() {
        let mut gallery = Gallery::new(vec![dummy("A", "a")]);
        let esc = Event::Key(crossterm::event::KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE));
        let msgs = gallery.on_event(&esc).await.unwrap();
        assert!(matches!(msgs[0], GalleryMessage::Quit));
    }

    #[tokio::test]
    async fn tab_switches_focus() {
        let mut gallery = Gallery::new(vec![dummy("A", "a")]);
        assert!(gallery.split.is_left_focused());

        let tab = Event::Key(crossterm::event::KeyEvent::new(KeyCode::Tab, crossterm::event::KeyModifiers::NONE));
        gallery.on_event(&tab).await;
        assert!(!gallery.split.is_left_focused());
    }
}
