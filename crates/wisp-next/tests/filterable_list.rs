use wisp_next::filterable_list::FilterableList;

#[test]
fn filters_once_per_query_change_and_selects_from_cached_matches() {
    let mut list =
        FilterableList::new(vec!["alpha".to_string(), "bravo".to_string(), "alphabet".to_string()], |entry| {
            entry.clone()
        });

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
    let mut list = FilterableList::new(vec!["one".to_string(), "two".to_string()], |entry| entry.clone());

    list.set_query("missing");
    assert_eq!(list.filtered_len(), 0);
    assert_eq!(list.selected_entry(), None);

    list.set_query("");
    assert_eq!(list.filtered_len(), 2);
    assert_eq!(list.selected_entry().map(String::as_str), Some("one"));
}
