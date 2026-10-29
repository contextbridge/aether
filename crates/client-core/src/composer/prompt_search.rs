use acp_utils::client::AcpClientError;
use acp_utils::notifications::{PromptSearchResponse, PromptSearchResult};

#[derive(Debug, Default)]
pub struct PromptSearchState {
    query: String,
    loading: bool,
    error: Option<AcpClientError>,
    results: Vec<PromptSearchResult>,
}

#[derive(Debug, Clone, Copy)]
pub enum PromptSearchStatus<'a> {
    Blank,
    Searching,
    Failed(&'a AcpClientError),
    NoMatches,
    Results,
}

impl PromptSearchState {
    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn results(&self) -> &[PromptSearchResult] {
        &self.results
    }

    pub fn status(&self) -> PromptSearchStatus<'_> {
        if let Some(error) = &self.error {
            return PromptSearchStatus::Failed(error);
        }
        if self.is_blank() {
            return PromptSearchStatus::Blank;
        }
        match (self.loading, self.results.is_empty()) {
            (true, true) => PromptSearchStatus::Searching,
            (false, true) => PromptSearchStatus::NoMatches,
            (_, false) => PromptSearchStatus::Results,
        }
    }

    pub(crate) fn set_query(&mut self, query: &str) -> bool {
        query.clone_into(&mut self.query);
        self.error = None;
        self.loading = !self.is_blank();
        if !self.loading {
            self.results.clear();
        }
        self.loading
    }

    pub(crate) fn on_response(&mut self, query: &str, response: Result<PromptSearchResponse, AcpClientError>) -> bool {
        if query != self.query {
            return false;
        }
        self.loading = false;
        match response {
            Ok(response) => {
                self.error = None;
                self.results = response.results;
            }
            Err(error) => {
                self.error = Some(error);
                self.results.clear();
            }
        }
        true
    }

    fn is_blank(&self) -> bool {
        self.query.trim().is_empty()
    }
}
