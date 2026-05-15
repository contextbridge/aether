use crate::components::ViewContext;
use crate::components::component::PickerMessage;
use crate::fuzzy_matcher::{FuzzyMatcher, Searchable};
use crate::line::Line;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::cmp::Ordering;

const DEFAULT_MAX_VISIBLE: usize = 10;

pub enum PickerKey {
    Escape,
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    Tab,
    BackTab,
    Confirm,
    Char(char),
    Backspace,
    BackspaceOnEmpty,
    ControlChar,
    Other,
}

#[doc = include_str!("docs/combobox.md")]
pub struct Combobox<T: Searchable + Send + Sync + 'static> {
    fuzzy: FuzzyMatcher<T>,
    selected_index: usize,
    scroll_offset: usize,
    max_visible: usize,
}

impl<T: Searchable + Send + Sync + 'static> Combobox<T> {
    pub fn new(items: Vec<T>) -> Self {
        Self { fuzzy: FuzzyMatcher::new(items), selected_index: 0, scroll_offset: 0, max_visible: DEFAULT_MAX_VISIBLE }
    }

    pub fn from_matches(matches: Vec<T>) -> Self {
        Self {
            fuzzy: FuzzyMatcher::from_matches(matches),
            selected_index: 0,
            scroll_offset: 0,
            max_visible: DEFAULT_MAX_VISIBLE,
        }
    }

    pub fn query(&self) -> &str {
        self.fuzzy.query()
    }

    pub fn matches(&self) -> &[T] {
        self.fuzzy.matches()
    }

    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    pub fn set_max_visible(&mut self, max: usize) {
        self.max_visible = max;
        self.ensure_visible();
    }

    pub fn set_match_sort(&mut self, sort: fn(&T, &T) -> Ordering) {
        self.fuzzy.set_match_sort(sort);
        self.scroll_offset = 0;
        if self.selected_index >= self.fuzzy.matches().len() {
            self.selected_index = 0;
        }
        self.ensure_visible();
    }

    pub fn is_empty(&self) -> bool {
        self.fuzzy.is_empty()
    }

    pub fn selected(&self) -> Option<&T> {
        self.fuzzy.matches().get(self.selected_index)
    }

    pub fn push_query_char(&mut self, c: char) {
        self.fuzzy.push_query_char(c);
        self.reset_viewport();
    }

    pub fn pop_query_char(&mut self) {
        if self.fuzzy.pop_query_char() {
            self.reset_viewport();
        }
    }

    pub fn set_selected_index(&mut self, index: usize) {
        let len = self.fuzzy.matches().len();
        if len == 0 {
            return;
        }
        self.selected_index = index.min(len - 1);
        self.ensure_visible();
    }

    pub fn move_up(&mut self) {
        self.move_up_where(|_| true);
    }

    pub fn move_down(&mut self) {
        self.move_down_where(|_| true);
    }

    pub fn move_up_where(&mut self, predicate: impl Fn(&T) -> bool) {
        let len = self.fuzzy.matches().len();
        if len == 0 {
            return;
        }
        let matches = self.fuzzy.matches();
        let mut idx = self.selected_index;
        for _ in 0..len {
            idx = if idx == 0 { len - 1 } else { idx - 1 };
            if predicate(&matches[idx]) {
                self.selected_index = idx;
                self.ensure_visible();
                return;
            }
        }
    }

    pub fn move_down_where(&mut self, predicate: impl Fn(&T) -> bool) {
        let len = self.fuzzy.matches().len();
        if len == 0 {
            return;
        }
        let matches = self.fuzzy.matches();
        let mut idx = self.selected_index;
        for _ in 0..len {
            idx = (idx + 1) % len;
            if predicate(&matches[idx]) {
                self.selected_index = idx;
                self.ensure_visible();
                return;
            }
        }
    }

    pub fn select_first_where(&mut self, predicate: impl Fn(&T) -> bool) {
        if let Some(idx) = self.fuzzy.matches().iter().position(&predicate) {
            self.selected_index = idx;
            self.ensure_visible();
        }
    }

    pub fn render_items(
        &self,
        context: &ViewContext,
        render_item: impl Fn(&T, bool, &ViewContext) -> Line,
    ) -> Vec<Line> {
        let inner = context.with_size((context.size.width.saturating_sub(2), context.size.height));
        self.visible_matches_with_selection()
            .into_iter()
            .map(|(item, is_selected)| render_item(item, is_selected, &inner).prepend("  "))
            .collect()
    }

    pub fn visible_matches_with_selection(&self) -> Vec<(&T, bool)> {
        let visible_selected_index = self.visible_selected_index();
        self.visible_matches().iter().enumerate().map(|(i, item)| (item, Some(i) == visible_selected_index)).collect()
    }

    /// Standard event dispatch for picker-style components.
    ///
    /// Handles Escape, Up/Down, Tab/Enter (confirm), Char (query + whitespace-close),
    /// Backspace, and `BackspaceOnEmpty`. Returns `PickerMessage<T>` for each action.
    pub fn handle_picker_event(&mut self, event: &crate::components::Event) -> Option<Vec<PickerMessage<T>>> {
        let crate::components::Event::Key(key_event) = event else {
            return None;
        };
        match classify_key(*key_event, self.fuzzy.query().is_empty()) {
            PickerKey::Escape => Some(vec![PickerMessage::Close]),
            PickerKey::BackspaceOnEmpty => Some(vec![PickerMessage::CloseAndPopChar]),
            PickerKey::MoveUp => {
                self.move_up();
                Some(vec![])
            }
            PickerKey::MoveDown => {
                self.move_down();
                Some(vec![])
            }
            PickerKey::Tab | PickerKey::Confirm => {
                if let Some(item) = self.selected().cloned() {
                    Some(vec![PickerMessage::Confirm(item)])
                } else {
                    Some(vec![PickerMessage::Close])
                }
            }
            PickerKey::Char(c) => {
                if c.is_whitespace() {
                    return Some(vec![PickerMessage::CloseWithChar(c)]);
                }
                self.push_query_char(c);
                Some(vec![PickerMessage::CharTyped(c)])
            }
            PickerKey::Backspace => {
                self.pop_query_char();
                Some(vec![PickerMessage::PopChar])
            }
            PickerKey::MoveLeft
            | PickerKey::MoveRight
            | PickerKey::BackTab
            | PickerKey::ControlChar
            | PickerKey::Other => Some(vec![]),
        }
    }

    fn visible_matches(&self) -> &[T] {
        let matches = self.fuzzy.matches();
        let end = (self.scroll_offset + self.max_visible).min(matches.len());
        &matches[self.scroll_offset..end]
    }

    fn visible_selected_index(&self) -> Option<usize> {
        self.selected_index.checked_sub(self.scroll_offset)
    }

    fn ensure_visible(&mut self) {
        if self.fuzzy.matches().is_empty() {
            self.scroll_offset = 0;
            return;
        }
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + self.max_visible {
            self.scroll_offset = self.selected_index + 1 - self.max_visible;
        }
    }

    fn reset_viewport(&mut self) {
        self.scroll_offset = 0;
        if self.selected_index >= self.fuzzy.matches().len() {
            self.selected_index = 0;
        }
    }
}

pub fn classify_key(key: KeyEvent, query_is_empty: bool) -> PickerKey {
    match key.code {
        KeyCode::Esc => PickerKey::Escape,
        KeyCode::Up => PickerKey::MoveUp,
        KeyCode::Down => PickerKey::MoveDown,
        KeyCode::Left => PickerKey::MoveLeft,
        KeyCode::Right => PickerKey::MoveRight,
        KeyCode::Tab => PickerKey::Tab,
        KeyCode::BackTab => PickerKey::BackTab,
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => PickerKey::MoveUp,
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => PickerKey::MoveDown,
        KeyCode::Enter => PickerKey::Confirm,
        KeyCode::Char(c) if c.is_control() => PickerKey::ControlChar,
        KeyCode::Char(c) => PickerKey::Char(c),
        KeyCode::Backspace if query_is_empty => PickerKey::BackspaceOnEmpty,
        KeyCode::Backspace => PickerKey::Backspace,
        _ => PickerKey::Other,
    }
}
