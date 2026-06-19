use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Nucleo};
use std::cmp::Ordering;
use std::sync::Arc;

pub trait Searchable: Clone {
    fn search_text(&self) -> String;
}

const MAX_MATCHES: u32 = 200;
const MATCH_TIMEOUT_MS: u64 = 10;
const MAX_TICKS_PER_QUERY: usize = 4;

pub struct FuzzyMatcher<T: Searchable + Send + Sync + 'static> {
    query: String,
    matches: Vec<T>,
    matcher: Nucleo<T>,
    match_sort: Option<fn(&T, &T) -> Ordering>,
}

impl<T: Searchable + Send + Sync + 'static> FuzzyMatcher<T> {
    pub fn new(items: Vec<T>) -> Self {
        let mut matcher = Nucleo::new(Config::DEFAULT, Arc::new(|| {}), Some(1), 1);
        let injector = matcher.injector();
        for item in items {
            injector.push(item, |item, columns| {
                let text = item.search_text();
                columns[0] = text.as_str().into();
            });
        }
        let _ = matcher.tick(0);

        let mut fuzzy = Self { query: String::new(), matches: Vec::new(), matcher, match_sort: None };
        fuzzy.matches = fuzzy.search(false);
        fuzzy
    }

    /// Creates a `FuzzyMatcher` with pre-populated matches (no Nucleo indexing).
    pub fn from_matches(matches: Vec<T>) -> Self {
        let nucleo = Nucleo::new(Config::DEFAULT, Arc::new(|| {}), Some(1), 1);
        Self { query: String::new(), matches, matcher: nucleo, match_sort: None }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn matches(&self) -> &[T] {
        &self.matches
    }

    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    pub fn set_match_sort(&mut self, sort: fn(&T, &T) -> Ordering) {
        self.match_sort = Some(sort);
        self.matches = self.search(false);
    }

    pub fn push_query_char(&mut self, c: char) {
        self.query.push(c);
        self.refresh_matches(true);
    }

    pub fn pop_query_char(&mut self) -> bool {
        if self.query.pop().is_none() {
            return false;
        }
        self.refresh_matches(false);
        true
    }

    /// Re-runs the search and updates the stored matches. Returns true if the
    /// match count changed (callers may need to clamp selection indices).
    pub fn refresh_matches(&mut self, append: bool) {
        self.matches = self.search(append);
    }

    fn search(&mut self, append: bool) -> Vec<T> {
        self.matcher.pattern.reparse(0, &self.query, CaseMatching::Smart, Normalization::Smart, append);
        let mut status = self.matcher.tick(MATCH_TIMEOUT_MS);
        let mut ticks = 0;
        while status.running && ticks < MAX_TICKS_PER_QUERY {
            status = self.matcher.tick(MATCH_TIMEOUT_MS);
            ticks += 1;
        }

        let snapshot = self.matcher.snapshot();
        let limit = snapshot.matched_item_count().min(MAX_MATCHES);
        let mut matches: Vec<T> = snapshot.matched_items(0..limit).map(|item| item.data.clone()).collect();
        if let Some(sort) = self.match_sort {
            matches.sort_by(sort);
        }
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct FakeItem {
        text: String,
    }

    impl FakeItem {
        fn new(text: &str) -> Self {
            Self { text: text.to_string() }
        }
    }

    impl Searchable for FakeItem {
        fn search_text(&self) -> String {
            self.text.clone()
        }
    }

    #[test]
    fn new_returns_all_items_with_empty_query() {
        let items = vec![FakeItem::new("alpha"), FakeItem::new("beta"), FakeItem::new("gamma")];
        let matcher = FuzzyMatcher::new(items);
        assert_eq!(matcher.matches().len(), 3);
        assert_eq!(matcher.query(), "");
    }

    #[test]
    fn push_query_char_filters_matches() {
        let items = vec![FakeItem::new("apple"), FakeItem::new("banana"), FakeItem::new("avocado")];
        let mut matcher = FuzzyMatcher::new(items);
        for c in "ban".chars() {
            matcher.push_query_char(c);
        }
        assert_eq!(matcher.matches().len(), 1);
        assert_eq!(matcher.matches()[0].text, "banana");
    }

    #[test]
    fn push_and_pop_query_char() {
        let items = vec![FakeItem::new("cat"), FakeItem::new("car"), FakeItem::new("dog")];
        let mut matcher = FuzzyMatcher::new(items);
        matcher.push_query_char('c');
        assert_eq!(matcher.query(), "c");
        matcher.push_query_char('a');
        assert_eq!(matcher.query(), "ca");

        matcher.pop_query_char();
        assert_eq!(matcher.query(), "c");
        matcher.pop_query_char();
        assert_eq!(matcher.query(), "");

        // pop on empty is no-op
        matcher.pop_query_char();
        assert_eq!(matcher.query(), "");
    }

    #[test]
    fn from_matches_populates_directly() {
        let items = vec![FakeItem::new("pre-populated")];
        let fuzzy = FuzzyMatcher::from_matches(items);
        assert_eq!(fuzzy.matches().len(), 1);
    }

    #[test]
    fn is_empty_reflects_match_state() {
        let empty: FuzzyMatcher<FakeItem> = FuzzyMatcher::from_matches(vec![]);
        assert!(empty.is_empty());

        let non_empty = FuzzyMatcher::from_matches(vec![FakeItem::new("a")]);
        assert!(!non_empty.is_empty());
    }

    #[test]
    fn set_match_sort_reorders_matches() {
        let items = vec![FakeItem::new("banana"), FakeItem::new("apple"), FakeItem::new("cherry")];
        let mut matcher = FuzzyMatcher::new(items);
        matcher.set_match_sort(|a, b| a.text.cmp(&b.text));
        assert_eq!(matcher.matches()[0].text, "apple");
        assert_eq!(matcher.matches()[1].text, "banana");
        assert_eq!(matcher.matches()[2].text, "cherry");
    }
}
