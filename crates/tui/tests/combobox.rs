use tui::testing::render_lines;
use tui::{Combobox, Line, PickerKey, PickerMessage, Searchable, ViewContext, classify_key};
use tui::{Event, KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, PartialEq)]
struct FakeItem {
    text: String,
    disabled: bool,
}

impl FakeItem {
    fn new(text: &str) -> Self {
        Self { text: text.to_string(), disabled: false }
    }

    fn disabled(text: &str) -> Self {
        Self { text: text.to_string(), disabled: true }
    }
}

impl Searchable for FakeItem {
    fn search_text(&self) -> String {
        self.text.clone()
    }
}

fn items(names: &[&str]) -> Vec<FakeItem> {
    names.iter().map(|n| FakeItem::new(n)).collect()
}

fn many_items(n: usize) -> Vec<FakeItem> {
    (0..n).map(|i| FakeItem::new(&format!("item-{i}"))).collect()
}

fn combo(names: &[&str]) -> Combobox<FakeItem> {
    Combobox::from_matches(items(names))
}

fn type_query(combobox: &mut Combobox<FakeItem>, s: &str) {
    for c in s.chars() {
        combobox.push_query_char(c);
    }
}

fn visible_texts(combobox: &Combobox<FakeItem>) -> Vec<String> {
    combobox.visible_matches_with_selection().iter().map(|(item, _)| item.text.clone()).collect()
}

fn selected_visible_pos(combobox: &Combobox<FakeItem>) -> usize {
    combobox.visible_matches_with_selection().iter().position(|(_, sel)| *sel).unwrap()
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

#[test]
fn new_returns_all_items_with_empty_query() {
    let combobox = Combobox::new(items(&["alpha", "beta", "gamma"]));
    assert_eq!(combobox.matches().len(), 3);
    assert_eq!(combobox.query(), "");
    assert_eq!(combobox.selected_index(), 0);
}

#[test]
fn push_query_char_filters_matches() {
    let mut combobox = Combobox::new(items(&["apple", "banana", "avocado"]));
    type_query(&mut combobox, "ban");
    assert_eq!(combobox.matches().len(), 1);
    assert_eq!(combobox.matches()[0].text, "banana");
}

#[test]
fn push_query_char_clamps_selected_index() {
    let mut combobox = Combobox::new(items(&["a", "b", "c"]));
    combobox.set_selected_index(2);
    combobox.push_query_char('a');
    assert_eq!(combobox.selected_index(), 0);
}

#[test]
fn push_and_pop_query_char() {
    let mut combobox = Combobox::new(items(&["cat", "car", "dog"]));
    for (action, expected) in [(Some('c'), "c"), (Some('a'), "ca"), (None, "c"), (None, "")] {
        match action {
            Some(c) => combobox.push_query_char(c),
            None => combobox.pop_query_char(),
        }
        assert_eq!(combobox.query(), expected);
    }
    // pop on empty is no-op
    combobox.pop_query_char();
    assert_eq!(combobox.query(), "");
}

#[test]
fn selection_wraps_around() {
    let mut combobox = combo(&["a", "b", "c"]);
    combobox.move_up();
    assert_eq!(combobox.selected_index(), 2);
    combobox.move_down();
    assert_eq!(combobox.selected_index(), 0);
}

#[test]
fn selected_returns_current_match() {
    let mut combobox = combo(&["x", "y"]);
    assert_eq!(combobox.selected().unwrap().text, "x");
    combobox.move_down();
    assert_eq!(combobox.selected().unwrap().text, "y");
}

#[test]
fn from_matches_populates_directly() {
    let combobox = combo(&["pre-populated"]);
    assert_eq!(combobox.matches().len(), 1);
    assert_eq!(combobox.selected_index(), 0);
}

#[test]
fn empty_matches_selection_is_noop() {
    let mut combobox: Combobox<FakeItem> = Combobox::from_matches(vec![]);
    combobox.move_up();
    combobox.move_down();
    assert!(combobox.selected().is_none());
}

#[test]
fn from_matches_stores_more_than_viewport() {
    assert_eq!(Combobox::from_matches(many_items(25)).matches().len(), 25);
}

#[test]
fn visible_matches_returns_viewport_window() {
    let combobox = Combobox::from_matches(many_items(25));
    let vis = visible_texts(&combobox);
    assert_eq!(vis.len(), 10);
    assert_eq!(vis[0], "item-0");
    assert_eq!(vis[9], "item-9");
}

#[test]
fn visible_matches_returns_all_when_fewer_than_viewport() {
    assert_eq!(Combobox::from_matches(many_items(3)).visible_matches_with_selection().len(), 3);
}

#[test]
fn scroll_down_past_viewport_adjusts_offset() {
    let mut combobox = Combobox::from_matches(many_items(25));
    for _ in 0..12 {
        combobox.move_down();
    }
    assert_eq!(combobox.selected_index(), 12);
    assert_eq!(visible_texts(&combobox)[0], "item-3");
    assert_eq!(selected_visible_pos(&combobox), 9);
}

#[test]
fn scroll_up_adjusts_offset() {
    let mut combobox = Combobox::from_matches(many_items(25));
    for _ in 0..15 {
        combobox.move_down();
    }
    for _ in 0..10 {
        combobox.move_up();
    }
    assert_eq!(combobox.selected_index(), 5);
    assert_eq!(selected_visible_pos(&combobox), 0);
}

#[test]
fn wrap_down_resets_scroll_offset() {
    let mut combobox = Combobox::from_matches(many_items(25));
    for _ in 0..24 {
        combobox.move_down();
    }
    assert_eq!(combobox.selected_index(), 24);
    combobox.move_down();
    assert_eq!(combobox.selected_index(), 0);
    assert_eq!(visible_texts(&combobox)[0], "item-0");
}

#[test]
fn wrap_up_scrolls_to_end() {
    let mut combobox = Combobox::from_matches(many_items(25));
    combobox.move_up();
    assert_eq!(combobox.selected_index(), 24);
    assert_eq!(selected_visible_pos(&combobox), 9);
}

#[test]
fn set_selected_index_clamps_and_ensures_visible() {
    let mut combobox = Combobox::from_matches(many_items(25));
    combobox.set_selected_index(100);
    assert_eq!(combobox.selected_index(), 24);
    combobox.set_selected_index(0);
    assert_eq!(combobox.selected_index(), 0);
}

#[test]
fn set_selected_index_noop_on_empty() {
    let mut combobox: Combobox<FakeItem> = Combobox::from_matches(vec![]);
    combobox.set_selected_index(5);
    assert_eq!(combobox.selected_index(), 0);
}

#[test]
fn classify_key_mappings() {
    let cases: Vec<(KeyEvent, bool, &str)> = vec![
        (key(KeyCode::Esc), true, "Escape"),
        (key(KeyCode::Up), true, "MoveUp"),
        (key(KeyCode::Down), true, "MoveDown"),
        (ctrl('p'), true, "MoveUp"),
        (ctrl('n'), true, "MoveDown"),
        (key(KeyCode::Enter), true, "Confirm"),
        (key(KeyCode::Char('a')), true, "Char"),
        (key(KeyCode::Backspace), true, "BackspaceOnEmpty"),
        (key(KeyCode::Backspace), false, "Backspace"),
        (key(KeyCode::Left), true, "MoveLeft"),
        (key(KeyCode::Right), true, "MoveRight"),
        (key(KeyCode::Tab), true, "Tab"),
        (key(KeyCode::BackTab), true, "BackTab"),
        (key(KeyCode::Home), true, "Other"),
    ];
    for (key_event, query_empty, expected) in cases {
        let result = classify_key(key_event, query_empty);
        let label = match result {
            PickerKey::Escape => "Escape",
            PickerKey::MoveUp => "MoveUp",
            PickerKey::MoveDown => "MoveDown",
            PickerKey::MoveLeft => "MoveLeft",
            PickerKey::MoveRight => "MoveRight",
            PickerKey::Tab => "Tab",
            PickerKey::BackTab => "BackTab",
            PickerKey::Confirm => "Confirm",
            PickerKey::Char(_) => "Char",
            PickerKey::Backspace => "Backspace",
            PickerKey::BackspaceOnEmpty => "BackspaceOnEmpty",
            PickerKey::ControlChar => "ControlChar",
            PickerKey::Other => "Other",
        };
        assert_eq!(label, expected, "classify_key({key_event:?}, {query_empty}) = {label}, expected {expected}");
    }
}

#[test]
fn render_items_empty_returns_empty() {
    let combobox: Combobox<FakeItem> = Combobox::from_matches(vec![]);
    let context = ViewContext::new((120, 40));
    let lines = combobox.render_items(&context, |_, _, _| Line::new("x".to_string()));
    assert!(lines.is_empty());
}

#[test]
fn render_items_calls_closure_for_each_visible() {
    let combobox = combo(&["a", "b", "c"]);
    let context = ViewContext::new((120, 40));
    let lines = combobox.render_items(&context, |item, _selected, _ctx| Line::new(item.text.clone()));
    let output = render_lines(&lines, 120, 3).get_lines();
    assert_eq!(output.len(), 3);
    for (line, expected) in output.iter().zip(["a", "b", "c"]) {
        assert!(line.contains(expected));
    }
}

#[test]
fn set_max_visible_changes_viewport_size() {
    let mut combobox = Combobox::from_matches(many_items(25));
    assert_eq!(combobox.visible_matches_with_selection().len(), 10);
    combobox.set_max_visible(5);
    assert_eq!(combobox.visible_matches_with_selection().len(), 5);
    combobox.set_max_visible(30);
    assert_eq!(combobox.visible_matches_with_selection().len(), 25);
}

fn disabled_items() -> Vec<FakeItem> {
    vec![FakeItem::new("a"), FakeItem::disabled("b"), FakeItem::new("c")]
}

#[test]
fn move_down_where_skips_disabled() {
    let mut combobox = Combobox::from_matches(disabled_items());
    combobox.move_down_where(|item| !item.disabled);
    assert_eq!(combobox.selected_index(), 2);
}

#[test]
fn move_up_where_skips_disabled() {
    let mut combobox = Combobox::from_matches(disabled_items());
    combobox.set_selected_index(2);
    combobox.move_up_where(|item| !item.disabled);
    assert_eq!(combobox.selected_index(), 0);
}

#[test]
fn select_first_where_finds_first_enabled() {
    let items = vec![FakeItem::disabled("a"), FakeItem::disabled("b"), FakeItem::new("c")];
    let mut combobox = Combobox::from_matches(items);
    combobox.select_first_where(|item| !item.disabled);
    assert_eq!(combobox.selected_index(), 2);
}

#[test]
fn move_down_where_noop_when_all_filtered() {
    let items = vec![FakeItem::disabled("a"), FakeItem::disabled("b"), FakeItem::disabled("c")];
    let mut combobox = Combobox::from_matches(items);
    combobox.move_down_where(|item| !item.disabled);
    assert_eq!(combobox.selected_index(), 0);
}

#[test]
fn handle_picker_event_paste_appends_sanitized_text() {
    let mut combobox = combo(&["fix login", "add tests"]);
    let event = Event::Paste("fix \nlogin".into());

    let outcome = combobox.handle_picker_event(&event).expect("paste consumed");

    assert!(matches!(outcome.as_slice(), [PickerMessage::TextTyped(text)] if text == "fix login"));
    assert_eq!(combobox.query(), "fix login");
}

#[test]
fn handle_picker_event_whitespace_closes_by_default() {
    let mut combobox = combo(&["alpha", "beta"]);
    let event = Event::Key(key(KeyCode::Char(' ')));
    let outcome = combobox.handle_picker_event(&event).expect("space consumed");
    assert!(matches!(outcome.as_slice(), [PickerMessage::CloseWithChar(' ')]));
    assert_eq!(combobox.query(), "");
}

#[test]
fn handle_picker_event_whitespace_appended_when_disabled() {
    let mut combobox = combo(&["fix login", "add tests"]).close_on_whitespace(false);
    combobox.push_query_char('f');
    combobox.push_query_char('i');
    combobox.push_query_char('x');
    let event = Event::Key(key(KeyCode::Char(' ')));
    let outcome = combobox.handle_picker_event(&event).expect("space consumed");
    assert!(matches!(outcome.as_slice(), [PickerMessage::CharTyped(' ')]));
    assert_eq!(combobox.query(), "fix ");
}
