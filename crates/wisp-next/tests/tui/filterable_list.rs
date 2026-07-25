use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{ListItem, Widget};
use wisp_next::filterable_list::FilterableList;
use wisp_next::theme::Theme;

#[test]
fn filters_once_per_query_change_and_selects_from_cached_matches() {
    let mut list =
        FilterableList::new(vec!["alpha".to_string(), "bravo".to_string(), "alphabet".to_string()], Clone::clone);

    list.set_query("alp");
    assert_eq!(list.filtered_entries().map(|(_, entry)| entry.as_str()).collect::<Vec<_>>(), ["alpha", "alphabet"]);
    assert_eq!(list.selected_entry().map(String::as_str), Some("alpha"));

    list.select_next();
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
fn persists_native_list_offset_for_mouse_row_selection() {
    let mut list = FilterableList::new((0..10).map(|index| format!("item {index}")).collect(), Clone::clone);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 20, 4));

    for _ in 0..7 {
        list.select_next();
    }
    list.view(&Theme::default(), "empty", |entry| ListItem::new(entry.clone()))
        .bordered("items")
        .render(Rect::new(0, 0, 20, 4), &mut buffer);

    let offset = list.offset();
    assert!(offset > 0);
    list.select_row(0);
    assert_eq!(list.selected_entry().map(String::as_str), Some(format!("item {offset}").as_str()));
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
