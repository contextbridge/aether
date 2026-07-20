use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::StatefulWidget;
use wisp::testing::buffer_text;
use wisp::theme::Theme;
use wisp::view::list_view::ListView;
use wisp::view::selection::SelectionState;

#[test]
fn list_view_is_stateful_without_embedding_selection() {
    let theme = Theme::default();
    let mut state = SelectionState::new(20);
    state.select(Some(12), 20);
    let area = Rect::new(0, 0, 12, 4);
    let mut buffer = Buffer::empty(area);

    let view = ListView::lazy(20, |index| Line::raw(format!("row {index}")), &theme).bordered("Rows");
    StatefulWidget::render(view, area, &mut buffer, &mut state);

    assert_eq!(state.selected(), Some(12));
    assert!(state.offset() > 0);
    assert_eq!(state.rows_area().y, 1);
    assert!(buffer_text(&buffer).contains("row 12"));
}
