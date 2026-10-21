use acp_utils::notifications::{PromptSearchResponse, PromptSearchResult};

/// The query of an open prompt-history search and the rule for which agent
/// responses still answer it.
///
/// Searches run against the agent while the user keeps typing, so responses
/// can arrive for a query the user has since edited away from. Each response
/// echoes the query it answered; one that no longer matches is stale and
/// dropped.
#[derive(Debug, Default)]
pub struct PromptSearchState {
    query: String,
    loading: bool,
    error: Option<String>,
}

impl PromptSearchState {
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Whether a search for a non-blank query is still waiting on the agent.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Whether the query has nothing to search for, so there are no results.
    pub fn is_blank(&self) -> bool {
        self.query.trim().is_empty()
    }

    /// Appends `c` to the query, returning the query it produced.
    pub fn push_char(&mut self, c: char) -> String {
        self.query.push(c);
        self.refresh();
        self.query.clone()
    }

    /// Appends the printable part of `text` to the query, returning the query it
    /// produced. Pastes that are entirely control characters leave it unchanged.
    pub fn push_str(&mut self, text: &str) -> String {
        let sanitized: String = text.chars().filter(|c| !c.is_control()).collect();
        if !sanitized.is_empty() {
            self.query.push_str(&sanitized);
            self.refresh();
        }
        self.query.clone()
    }

    /// Replaces the query, as a search field does on every edit, returning the query it produced.
    pub fn set_query(&mut self, query: &str) -> String {
        query.clone_into(&mut self.query);
        self.refresh();
        self.query.clone()
    }

    /// Drops the last character of the query, returning the query it produced.
    pub fn backspace(&mut self) -> String {
        self.query.pop();
        self.refresh();
        self.query.clone()
    }

    /// The response's results when it answers the current query, `None` when
    /// it is stale.
    pub fn on_results(&mut self, response: PromptSearchResponse) -> Option<Vec<PromptSearchResult>> {
        if response.query != self.query {
            return None;
        }
        self.loading = false;
        self.error = None;
        Some(response.results)
    }

    /// Records a failed search, reporting whether it was for the current query.
    pub fn on_failed(&mut self, query: &str, error: String) -> bool {
        if query != self.query {
            return false;
        }
        self.loading = false;
        self.error = Some(error);
        true
    }

    fn refresh(&mut self) {
        self.error = None;
        self.loading = !self.is_blank();
    }
}
