use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::StatefulWidget;
use wisp::theme::Theme;
use wisp::view::filterable_list::FilterableList;
use wisp::view::selection::Direction;

#[test]
fn filters_once_per_query_change_and_selects_from_cached_matches() {
    let mut list =
        FilterableList::new(vec!["alpha".to_string(), "bravo".to_string(), "alphabet".to_string()], Clone::clone);

    list.set_query("alp");
    assert_eq!(list.filtered_entries().map(|(_, entry)| entry.as_str()).collect::<Vec<_>>(), ["alpha", "alphabet"]);
    assert_eq!(list.selected_entry().map(String::as_str), Some("alpha"));

    list.step(Direction::Forward);
    assert_eq!(list.selected_entry().map(String::as_str), Some("alphabet"));

    list.set_query("alp");
    assert_eq!(list.selected_entry().map(String::as_str), Some("alphabet"));

    list.set_query("br");
    assert_eq!(list.selected_entry().map(String::as_str), Some("bravo"));
}

#[test]
fn clearing_or_emptying_a_filter_keeps_selection_valid() {
    let mut list = FilterableList::new(vec!["one".to_string(), "two".to_string()], Clone::clone);

    list.set_query("missing");
    assert_eq!(list.filtered_len(), 0);
    assert_eq!(list.selected_entry(), None);

    list.set_query("");
    assert_eq!(list.filtered_len(), 2);
    assert_eq!(list.selected_entry().map(String::as_str), Some("one"));
}

#[test]
fn maps_a_click_to_the_row_drawn_under_it() {
    let area = Rect::new(0, 0, 20, 4);
    let mut list = FilterableList::new((0..10).map(|index| format!("item {index}")).collect(), Clone::clone);
    let mut buffer = Buffer::empty(area);

    for _ in 0..7 {
        list.step(Direction::Forward);
    }
    let theme = Theme::default();
    let (view, selection) = list.view(&theme, |entry| Line::raw(entry.clone()));
    let view = view.empty_message("empty").bordered("items");
    StatefulWidget::render(view, area, &mut buffer, selection);

    let offset = list.offset();
    assert!(offset > 0, "the selection should have scrolled the list");
    // Row 0 is the block's top border; the first entry is drawn on row 1.
    assert!(list.select_at(1));
    assert_eq!(list.selected_entry().map(String::as_str), Some(format!("item {offset}").as_str()));
}

#[test]
fn ignores_clicks_outside_the_rows_it_drew() {
    let area = Rect::new(0, 0, 20, 4);
    let mut list = FilterableList::new(vec!["only".to_string()], Clone::clone);
    let mut buffer = Buffer::empty(area);
    let theme = Theme::default();
    let (view, selection) = list.view(&theme, |entry| Line::raw(entry.clone()));
    let view = view.empty_message("empty").bordered("items");
    StatefulWidget::render(view, area, &mut buffer, selection);

    assert!(!list.select_at(0), "the top border is not a row");
    assert!(!list.select_at(3), "the bottom border is not a row");
    assert!(list.select_at(1));
}

#[test]
fn caches_normalized_search_keys_and_edits_its_own_query() {
    let mut list = FilterableList::new(vec!["Älpha".to_string(), "bravo".to_string()], Clone::clone);

    for character in "älp".chars() {
        list.push_query_char(character);
    }
    assert_eq!(list.query(), "älp");
    assert_eq!(list.selected_entry().map(String::as_str), Some("Älpha"));

    list.pop_query_char();
    assert_eq!(list.query(), "äl");
    assert_eq!(list.filtered_len(), 1);
}
