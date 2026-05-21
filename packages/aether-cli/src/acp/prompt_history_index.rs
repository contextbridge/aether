use super::session_store::SessionMeta;
use acp_utils::notifications::{PromptSearchParams, PromptSearchResponse, PromptSearchResult};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing::warn;

const PROMPT_HISTORY_MAX_ENTRIES: usize = 100;
const PROMPT_SEARCH_DEFAULT_LIMIT: usize = 20;
const PROMPT_SEARCH_MAX_LIMIT: usize = 50;

pub(super) struct PromptHistoryIndex {
    path: PathBuf,
    state: Mutex<State>,
}

impl PromptHistoryIndex {
    pub(super) fn new(path: PathBuf) -> Self {
        Self { path, state: Mutex::new(State::Unloaded) }
    }

    pub(super) fn append_prompt(&self, meta: &SessionMeta, prompt: String) -> io::Result<()> {
        let entry = PromptHistoryEntry::new(meta, prompt);
        let mut state = self.lock_state();
        let entries = self.ensure_loaded(&mut state)?;

        entries.push_back(entry);
        if entries.len() > PROMPT_HISTORY_MAX_ENTRIES {
            entries.pop_front();
            self.rewrite_file(entries.iter())
        } else {
            self.append_to_file(entries.back().expect("just pushed"))
        }
    }

    pub(super) fn search(&self, params: &PromptSearchParams) -> io::Result<PromptSearchResponse> {
        let query = params.query.trim();
        let limit = prompt_search_limit(params.limit);
        let mut state = self.lock_state();
        let entries = self.ensure_loaded(&mut state)?;

        let response = PromptSearchResponse { query: query.to_string(), results: Vec::new(), truncated: false };
        if query.is_empty() {
            return Ok(response);
        }

        let mut results: Vec<_> =
            entries.iter().rev().filter_map(|entry| entry.search_result(query)).take(limit + 1).collect();
        let truncated = results.len() > limit;
        if truncated {
            results.truncate(limit);
        }
        Ok(PromptSearchResponse { results, truncated, ..response })
    }

    pub(super) fn is_index_path(&self, path: &Path) -> bool {
        path == self.path
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn ensure_loaded<'a>(&self, state: &'a mut State) -> io::Result<&'a mut VecDeque<PromptHistoryEntry>> {
        if let State::Unloaded = state {
            let (entries, needs_rewrite) = self.read_disk_capped()?;
            *state = State::Loaded(entries);
            if needs_rewrite {
                let State::Loaded(entries) = state else { unreachable!() };
                self.rewrite_file(entries.iter())?;
            }
        }
        let State::Loaded(entries) = state else { unreachable!() };
        Ok(entries)
    }

    /// Read every entry from disk, keep at most `PROMPT_HISTORY_MAX_ENTRIES`,
    /// and report whether the on-disk file exceeded the cap (so the caller can
    /// compact it).
    fn read_disk_capped(&self) -> io::Result<(VecDeque<PromptHistoryEntry>, bool)> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((VecDeque::new(), false)),
            Err(e) => return Err(e),
        };

        let parsed: Vec<_> = BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .filter_map(|line| parse_prompt_history_line(&line))
            .collect();

        let needs_rewrite = parsed.len() > PROMPT_HISTORY_MAX_ENTRIES;
        let skip = parsed.len().saturating_sub(PROMPT_HISTORY_MAX_ENTRIES);
        Ok((parsed.into_iter().skip(skip).collect(), needs_rewrite))
    }

    fn append_to_file(&self, entry: &PromptHistoryEntry) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        let line = serde_json::to_string(entry)
            .map_err(|e| io::Error::other(format!("Failed to serialize prompt history entry: {e}")))?;
        writeln!(file, "{line}")
    }

    fn rewrite_file<'a>(&self, entries: impl IntoIterator<Item = &'a PromptHistoryEntry>) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp_path = self.path.with_extension("jsonl.tmp");
        {
            let mut file = File::create(&tmp_path)?;
            for entry in entries {
                let line = serde_json::to_string(entry)
                    .map_err(|e| io::Error::other(format!("Failed to serialize prompt history entry: {e}")))?;
                writeln!(file, "{line}")?;
            }
        }
        fs::rename(tmp_path, &self.path)
    }
}

enum State {
    Unloaded,
    Loaded(VecDeque<PromptHistoryEntry>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PromptHistoryEntry {
    session_id: String,
    cwd: PathBuf,
    session_created_at: String,
    prompt: String,
}

impl PromptHistoryEntry {
    fn new(meta: &SessionMeta, prompt: String) -> Self {
        Self {
            session_id: meta.session_id.clone(),
            cwd: meta.cwd.clone(),
            session_created_at: meta.created_at.clone(),
            prompt,
        }
    }

    fn search_result(&self, query: &str) -> Option<PromptSearchResult> {
        let (match_start, match_end) = find_prompt_match(&self.prompt, query)?;
        Some(PromptSearchResult {
            session_id: self.session_id.clone(),
            cwd: self.cwd.clone(),
            session_created_at: self.session_created_at.clone(),
            prompt: self.prompt.clone(),
            match_start,
            match_end,
        })
    }
}

fn prompt_search_limit(requested: Option<usize>) -> usize {
    requested.unwrap_or(PROMPT_SEARCH_DEFAULT_LIMIT).clamp(1, PROMPT_SEARCH_MAX_LIMIT)
}

fn parse_prompt_history_line(line: &str) -> Option<PromptHistoryEntry> {
    if line.trim().is_empty() {
        return None;
    }
    match serde_json::from_str(line) {
        Ok(entry) => Some(entry),
        Err(e) => {
            warn!("Skipping malformed prompt history line: {e}");
            None
        }
    }
}

fn find_prompt_match(prompt: &str, query: &str) -> Option<(usize, usize)> {
    // Smart case: a query with any uppercase char triggers an exact-substring
    // match; otherwise the comparison is case-insensitive.
    if query.chars().any(char::is_uppercase) {
        prompt.find(query).map(|start| (start, start + query.len()))
    } else {
        find_case_insensitive(prompt, query)
    }
}

/// Case-insensitive substring search that returns byte offsets into the
/// **original** `prompt`. Lowercasing can change byte length (e.g. 'İ' → "i̇"),
/// so we walk the original and lowercased strings in parallel to translate
/// the match position back.
fn find_case_insensitive(prompt: &str, query: &str) -> Option<(usize, usize)> {
    let lower_query = query.to_lowercase();
    let lower_prompt = prompt.to_lowercase();
    let lower_match = lower_prompt.find(&lower_query)?;
    let lower_match_end = lower_match + lower_query.len();

    let mut lower_offset = 0usize;
    let mut orig_start: Option<usize> = None;
    for (orig_idx, ch) in prompt.char_indices() {
        if lower_offset == lower_match && orig_start.is_none() {
            orig_start = Some(orig_idx);
        }
        if lower_offset >= lower_match_end {
            return orig_start.map(|s| (s, orig_idx));
        }
        lower_offset += ch.to_lowercase().map(char::len_utf8).sum::<usize>();
    }
    orig_start.map(|s| (s, prompt.len()))
}
