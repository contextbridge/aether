use super::*;
use crossterm::event::KeyCode;
use tui::{Color, Cursor, Either, Frame, Line, SplitLayout, SplitPanel, Style};

struct StubComponent {
    label: String,
    messages: Vec<String>,
}

impl StubComponent {
    fn new(label: &str) -> Self {
        Self { label: label.into(), messages: Vec::new() }
    }

    fn with_messages(label: &str, msgs: Vec<&str>) -> Self {
        Self { label: label.into(), messages: msgs.into_iter().map(Into::into).collect() }
    }
}

impl Component for StubComponent {
    type Message = String;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<String>> {
        if let Event::Key(_) = event {
            if self.messages.is_empty() { Some(vec![]) } else { Some(self.messages.clone()) }
        } else {
            None
        }
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let mut lines = vec![Line::new(&self.label)];
        while lines.len() < ctx.size.height as usize {
            lines.push(Line::default());
        }
        Frame::new(lines)
    }
}

fn make_split() -> SplitPanel<StubComponent, StubComponent> {
    SplitPanel::new(StubComponent::new("LEFT"), StubComponent::new("RIGHT"), SplitLayout::fixed(15))
}

struct WideComponent {
    text: &'static str,
}

impl Component for WideComponent {
    type Message = ();

    async fn on_event(&mut self, _: &Event) -> Option<Vec<()>> {
        None
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let mut lines = vec![Line::new(self.text)];
        while lines.len() < ctx.size.height as usize {
            lines.push(Line::default());
        }
        Frame::new(lines)
    }
}

struct StyledWideComponent {
    text: &'static str,
    style: Style,
}

impl Component for StyledWideComponent {
    type Message = ();

    async fn on_event(&mut self, _: &Event) -> Option<Vec<()>> {
        None
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let mut lines = vec![Line::with_style(self.text, self.style)];
        while lines.len() < ctx.size.height as usize {
            lines.push(Line::default());
        }
        Frame::new(lines)
    }
}

#[test]
fn renders_both_panels_side_by_side() {
    let mut split = make_split();
    let term = render_component(|ctx| split.render(ctx), 40, 3);
    assert_buffer_eq(&term, &["LEFT           RIGHT", "", ""]);
}

#[test]
fn renders_with_separator() {
    let mut split = make_split().with_separator("|", Style::default());
    let term = render_component(|ctx| split.render(ctx), 40, 3);
    assert_buffer_eq(&term, &["LEFT           |RIGHT", "               |", "               |"]);
}

#[test]
fn starts_with_left_focused() {
    let split = make_split();
    assert!(split.is_left_focused());
}

#[tokio::test]
async fn tab_switches_focus_to_right() {
    let mut split = make_split();
    assert!(split.is_left_focused());

    split.on_event(&Event::Key(key(KeyCode::Tab))).await;
    assert!(!split.is_left_focused());
}

#[tokio::test]
async fn backtab_switches_focus_to_left() {
    let mut split = make_split();
    split.focus_right();
    assert!(!split.is_left_focused());

    split.on_event(&Event::Key(key(KeyCode::BackTab))).await;
    assert!(split.is_left_focused());
}

#[tokio::test]
async fn routes_events_to_focused_child() {
    let mut split = SplitPanel::new(
        StubComponent::with_messages("L", vec!["from_left"]),
        StubComponent::with_messages("R", vec!["from_right"]),
        SplitLayout::fixed(10),
    );

    let result = split.on_event(&Event::Key(key(KeyCode::Char('a')))).await.unwrap();
    assert_eq!(result.len(), 1);
    assert!(matches!(&result[0], Either::Left(s) if s == "from_left"));

    split.focus_right();
    let result = split.on_event(&Event::Key(key(KeyCode::Char('a')))).await.unwrap();
    assert_eq!(result.len(), 1);
    assert!(matches!(&result[0], Either::Right(s) if s == "from_right"));
}

#[tokio::test]
async fn resize_keys_widen_left_panel() {
    let mut split = make_split().with_resize_keys();

    let term = render_component(|ctx| split.render(ctx), 40, 1);
    assert_buffer_eq(&term, &["LEFT           RIGHT"]);

    split.on_event(&Event::Key(key(KeyCode::Char('>')))).await;

    let term = render_component(|ctx| split.render(ctx), 40, 1);
    assert_buffer_eq(&term, &["LEFT               RIGHT"]);
}

#[tokio::test]
async fn resize_keys_narrow_left_panel() {
    let mut split = make_split().with_resize_keys();

    split.on_event(&Event::Key(key(KeyCode::Char('>')))).await;
    split.on_event(&Event::Key(key(KeyCode::Char('<')))).await;

    let term = render_component(|ctx| split.render(ctx), 40, 1);
    assert_buffer_eq(&term, &["LEFT           RIGHT"]);
}

#[tokio::test]
async fn resize_keys_disabled_by_default() {
    let mut split = SplitPanel::new(
        StubComponent::with_messages("L", vec!["got_it"]),
        StubComponent::new("R"),
        SplitLayout::fixed(10),
    );

    let result = split.on_event(&Event::Key(key(KeyCode::Char('>')))).await.unwrap();
    assert!(matches!(&result[0], Either::Left(s) if s == "got_it"));

    let term = render_component(|ctx| split.render(ctx), 40, 1);
    assert_buffer_eq(&term, &["L           R"]);
}

#[test]
fn cursor_from_right_panel_is_offset_by_left_width() {
    struct CursorComponent;
    impl Component for CursorComponent {
        type Message = ();
        async fn on_event(&mut self, _: &Event) -> Option<Vec<()>> {
            None
        }
        fn render(&mut self, _ctx: &ViewContext) -> Frame {
            Frame::new(vec![Line::new("input")]).with_cursor(Cursor::visible(0, 3))
        }
    }

    let mut split = SplitPanel::new(StubComponent::new("L"), CursorComponent, SplitLayout::fixed(15));
    split.focus_right();

    let ctx = ViewContext::new((40, 3));
    let frame = split.render(&ctx);
    let cursor = frame.cursor();
    assert!(cursor.is_visible);
    assert_eq!(cursor.row, 0);
    assert_eq!(cursor.col, 3 + 15);
}

#[test]
fn cursor_from_wrapped_right_panel_accounts_for_wrap_row_and_offset() {
    struct CursorComponent;
    impl Component for CursorComponent {
        type Message = ();
        async fn on_event(&mut self, _: &Event) -> Option<Vec<()>> {
            None
        }
        fn render(&mut self, _ctx: &ViewContext) -> Frame {
            Frame::new(vec![Line::new("1234567890ABCDEFGHIJ")]).with_cursor(Cursor::visible(0, 19))
        }
    }

    let mut split = SplitPanel::new(StubComponent::new("LEFT"), CursorComponent, SplitLayout::fixed(12))
        .with_separator("|", Style::default());
    split.focus_right();

    let ctx = ViewContext::new((30, 3));
    let frame = split.render(&ctx);
    let cursor = frame.cursor();

    assert!(cursor.is_visible);
    assert_eq!(cursor.row, 1);
    assert_eq!(cursor.col, 12 + 1 + 2);
}

#[test]
fn cursor_from_left_panel_is_not_offset() {
    struct CursorComponent;
    impl Component for CursorComponent {
        type Message = ();
        async fn on_event(&mut self, _: &Event) -> Option<Vec<()>> {
            None
        }
        fn render(&mut self, _ctx: &ViewContext) -> Frame {
            Frame::new(vec![Line::new("input")]).with_cursor(Cursor::visible(0, 5))
        }
    }

    let mut split = SplitPanel::new(CursorComponent, StubComponent::new("R"), SplitLayout::fixed(10));
    let ctx = ViewContext::new((40, 3));
    let frame = split.render(&ctx);
    let cursor = frame.cursor();
    assert!(cursor.is_visible);
    assert_eq!(cursor.col, 5);
}

#[test]
fn soft_wraps_right_panel_to_its_allocated_width() {
    let mut split = SplitPanel::new(
        StubComponent::new("LEFT"),
        WideComponent { text: "1234567890ABCDEFGHIJ" },
        SplitLayout::fixed(12),
    )
    .with_separator("|", Style::default());

    let term = render_component(|ctx| split.render(ctx), 30, 3);

    assert_buffer_eq(&term, &["LEFT        |1234567890ABCDEFG", "            |HIJ", "            |"]);
}

#[test]
fn soft_wrap_preserves_right_panel_background_style_on_wrapped_rows() {
    let mut split = SplitPanel::new(
        StubComponent::new("LEFT"),
        StyledWideComponent { text: "1234567890ABCDEFGHIJ", style: Style::default().bg_color(Color::Blue) },
        SplitLayout::fixed(12),
    )
    .with_separator("|", Style::default());

    let term = render_component(|ctx| split.render(ctx), 30, 3);

    // right panel starts at col 13 (left=12 + separator=1)
    let expected_bg = term.get_style_at(0, 13).bg;
    assert_eq!(term.get_style_at(1, 13).bg, expected_bg);
    assert_eq!(term.get_style_at(1, 29).bg, expected_bg);
}

#[test]
fn left_child_wider_than_allocation_does_not_bleed_into_right_pane() {
    let mut split = SplitPanel::new(
        WideComponent { text: "1234567890ABCDEFGHIJ" },
        StubComponent::new("RIGHT"),
        SplitLayout::fixed(12),
    )
    .with_separator("|", Style::default());

    let term = render_component(|ctx| split.render(ctx), 30, 3);

    // left allocation is 12 columns; the wide left content must wrap inside
    // those 12 columns and never overlap the separator or the right pane.
    assert_buffer_eq(&term, &["1234567890AB|RIGHT", "CDEFGHIJ    |", "            |"]);
}

#[test]
fn render_emits_exactly_context_height_rows() {
    let mut split = SplitPanel::new(
        WideComponent { text: "1234567890ABCDEFGHIJKLMNOPQRSTUV" },
        StubComponent::new("R"),
        SplitLayout::fixed(15),
    );
    let ctx = ViewContext::new((40, 4));
    let frame = split.render(&ctx);
    assert_eq!(frame.lines().len(), 4, "split panel must always emit ctx.region.height rows");
}

#[test]
fn render_pads_to_context_height_when_children_are_shorter() {
    let mut split = make_split();
    let ctx = ViewContext::new((40, 6));
    let frame = split.render(&ctx);
    assert_eq!(frame.lines().len(), 6);
}

#[test]
fn cursor_is_hidden_when_focused_child_cursor_falls_beyond_truncation() {
    struct DeepCursorComponent;
    impl Component for DeepCursorComponent {
        type Message = ();
        async fn on_event(&mut self, _: &Event) -> Option<Vec<()>> {
            None
        }
        fn render(&mut self, _ctx: &ViewContext) -> Frame {
            let lines: Vec<Line> = (0..10).map(|i| Line::new(format!("row{i}"))).collect();
            Frame::new(lines).with_cursor(Cursor::visible(8, 0))
        }
    }

    let mut split = SplitPanel::new(StubComponent::new("L"), DeepCursorComponent, SplitLayout::fixed(15));
    split.focus_right();

    let ctx = ViewContext::new((40, 4));
    let frame = split.render(&ctx);
    assert!(!frame.cursor().is_visible, "cursor at row 8 must be hidden after truncation to 4 rows");
}

#[test]
fn cursor_remains_visible_when_focused_child_cursor_in_visible_range() {
    struct CursorComponent;
    impl Component for CursorComponent {
        type Message = ();
        async fn on_event(&mut self, _: &Event) -> Option<Vec<()>> {
            None
        }
        fn render(&mut self, _ctx: &ViewContext) -> Frame {
            let lines: Vec<Line> = (0..10).map(|i| Line::new(format!("row{i}"))).collect();
            Frame::new(lines).with_cursor(Cursor::visible(2, 1))
        }
    }

    let mut split = SplitPanel::new(StubComponent::new("L"), CursorComponent, SplitLayout::fixed(15));
    split.focus_right();

    let ctx = ViewContext::new((40, 4));
    let frame = split.render(&ctx);
    assert!(frame.cursor().is_visible);
    assert_eq!(frame.cursor().row, 2);
    assert_eq!(frame.cursor().col, 1 + 15);
}

#[tokio::test]
async fn focus_left_and_focus_right() {
    let mut split = make_split();
    assert!(split.is_left_focused());

    split.focus_right();
    assert!(!split.is_left_focused());

    split.focus_left();
    assert!(split.is_left_focused());
}
