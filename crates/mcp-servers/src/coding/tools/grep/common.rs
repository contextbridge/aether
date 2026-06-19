use grep::searcher::{Sink, SinkMatch};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum OutputMode {
    /// Return matching lines with file paths and line numbers
    Content,
    /// Return only file paths that contain matches
    #[serde(alias = "files_with_matches")]
    FilesWithMatches,
    /// Return match counts per file
    Count,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MatchData {
    pub file: String,
    pub line_number: Option<usize>,
    pub line: String,
    pub before_context: Option<Vec<String>>,
    pub after_context: Option<Vec<String>>,
}

pub struct MatchCollectorSink {
    file_path: std::path::PathBuf,
    line_numbers: bool,
    max_results: Option<usize>,
    pub matches: Vec<MatchData>,
    context_before: Vec<String>,
}

impl MatchCollectorSink {
    pub fn new(file_path: &Path, line_numbers: bool) -> Self {
        Self {
            file_path: file_path.to_path_buf(),
            line_numbers,
            max_results: None,
            matches: Vec::new(),
            context_before: Vec::new(),
        }
    }

    pub fn with_max_results(file_path: &Path, line_numbers: bool, max_results: Option<usize>) -> Self {
        Self {
            file_path: file_path.to_path_buf(),
            line_numbers,
            max_results,
            matches: Vec::new(),
            context_before: Vec::new(),
        }
    }
}

impl Sink for MatchCollectorSink {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &grep::searcher::Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        // Check if we've hit the max results limit
        if let Some(max) = self.max_results
            && self.matches.len() >= max
        {
            return Ok(false); // Stop searching
        }

        let line_str = std::str::from_utf8(mat.bytes()).unwrap_or("<invalid utf8>");

        let before_context = if self.context_before.is_empty() { None } else { Some(self.context_before.clone()) };

        let match_data = MatchData {
            file: self.file_path.display().to_string(),
            line_number: if self.line_numbers { mat.line_number().and_then(|n| usize::try_from(n).ok()) } else { None },
            line: line_str.to_string(),
            before_context,
            after_context: None, // Will be filled by context method
        };

        self.matches.push(match_data);
        self.context_before.clear();
        Ok(true)
    }

    fn context(
        &mut self,
        _searcher: &grep::searcher::Searcher,
        ctx: &grep::searcher::SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        let line_str = std::str::from_utf8(ctx.bytes()).unwrap_or("<invalid utf8>");

        match ctx.kind() {
            grep::searcher::SinkContextKind::Before => {
                self.context_before.push(line_str.to_string());
            }
            grep::searcher::SinkContextKind::After => {
                // Add after context to the last match
                if let Some(last_match) = self.matches.last_mut() {
                    if last_match.after_context.is_none() {
                        last_match.after_context = Some(Vec::new());
                    }
                    if let Some(ref mut after) = last_match.after_context {
                        after.push(line_str.to_string());
                    }
                }
            }
            grep::searcher::SinkContextKind::Other => {}
        }

        Ok(true)
    }
}

pub struct HasMatchSink {
    pub has_match: bool,
}

impl HasMatchSink {
    pub fn new() -> Self {
        Self { has_match: false }
    }
}

impl Default for HasMatchSink {
    fn default() -> Self {
        Self::new()
    }
}

impl Sink for HasMatchSink {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &grep::searcher::Searcher, _mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        self.has_match = true;
        Ok(false)
    }
}

pub struct CountSink {
    pub count: usize,
}

impl CountSink {
    pub fn new() -> Self {
        Self { count: 0 }
    }
}

impl Default for CountSink {
    fn default() -> Self {
        Self::new()
    }
}

impl Sink for CountSink {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &grep::searcher::Searcher, _mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        self.count += 1;
        Ok(true)
    }
}
