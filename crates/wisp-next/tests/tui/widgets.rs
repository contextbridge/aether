use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{StatefulWidget, Widget};
use wisp_next::test_support::list_view::ListView;
use wisp_next::test_support::progress_indicator::{ProgressActivity, ProgressIndicator, ProgressIndicatorView};
use wisp_next::test_support::selection::SelectionState;
use wisp_next::test_support::theme::Theme;

fn buffer_text(buffer: &Buffer) -> String {
    (buffer.area.top()..buffer.area.bottom())
        .map(|y| {
            (buffer.area.left()..buffer.area.right())
                .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_string()))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

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

#[test]
fn progress_view_renders_from_external_indicator_state() {
    let theme = Theme::default();
    let mut indicator = ProgressIndicator::default();
    indicator.update(ProgressActivity { agent_busy: true, ..Default::default() }, 1);
    let view = ProgressIndicatorView::new(&indicator, &theme, 0);
    assert_eq!(view.line_count(), 3);

    let area = Rect::new(0, 0, 40, 3);
    let mut buffer = Buffer::empty(area);
    view.render(area, &mut buffer);
    assert!(buffer_text(&buffer).contains("Tip:"));
}
