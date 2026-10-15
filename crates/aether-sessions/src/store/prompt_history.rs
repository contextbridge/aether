use crate::SessionMeta;
use crate::store::append_to_jsonl;
use acp_utils::notifications::{PromptSearchResponse, PromptSearchResult};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use tracing::warn;
use utils::fs::write_atomic;

const PROMPT_HISTORY_MAX_ENTRIES: usize = 100;
const PROMPT_HISTORY_COMPACT_AFTER_LINES: usize = 2 * PROMPT_HISTORY_MAX_ENTRIES;
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
        let history = self.ensure_loaded(&mut state)?;

        append_to_jsonl(&self.path, &entry_line(&entry)?)?;
        history.file_lines += 1;
        history.entries.push_back(entry);
        if history.entries.len() > PROMPT_HISTORY_MAX_ENTRIES {
            history.entries.pop_front();
        }
        self.compact_if_full(history)
    }

    pub(super) fn rebuild<I: IntoIterator<Item = (SessionMeta, String)>>(&self, entries: I) -> io::Result<()> {
        let mut state = self.lock_state();
        let entries =
            keep_newest(entries.into_iter().map(|(meta, prompt)| PromptHistoryEntry::new(&meta, prompt)).collect());
        let mut history = LoadedHistory { entries, file_lines: 0 };
        self.rewrite_file(&mut history)?;
        *state = State::Loaded(history);
        Ok(())
    }

    pub(super) fn relocate_session(&self, session_id: &str, new_cwd: &Path) -> io::Result<()> {
        let mut state = self.lock_state();
        let history = self.ensure_loaded(&mut state)?;

        let mut changed = false;
        for entry in history.entries.iter_mut().filter(|entry| entry.session_id == session_id) {
            entry.cwd = new_cwd.to_path_buf();
            changed = true;
        }

        if changed { self.rewrite_file(history) } else { Ok(()) }
    }

    pub(super) fn search(&self, query: &str, limit: Option<usize>) -> io::Result<PromptSearchResponse> {
        let query = query.trim();
        let limit = prompt_search_limit(limit);
        let mut state = self.lock_state();
        let entries = &self.ensure_loaded(&mut state)?.entries;

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

    fn lock_state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn ensure_loaded<'a>(&self, state: &'a mut State) -> io::Result<&'a mut LoadedHistory> {
        if let State::Unloaded = state {
            let mut history = self.load_history()?;
            self.compact_if_full(&mut history)?;
            *state = State::Loaded(history);
        }
        let State::Loaded(history) = state else { unreachable!() };
        Ok(history)
    }

    fn load_history(&self) -> io::Result<LoadedHistory> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(LoadedHistory { entries: VecDeque::new(), file_lines: 0 });
            }
            Err(error) => return Err(error),
        };

        let parsed: Vec<_> = BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .filter_map(|line| parse_prompt_history_line(&line))
            .collect();

        Ok(LoadedHistory { file_lines: parsed.len(), entries: keep_newest(parsed) })
    }

    fn compact_if_full(&self, history: &mut LoadedHistory) -> io::Result<()> {
        if history.file_lines > PROMPT_HISTORY_COMPACT_AFTER_LINES { self.rewrite_file(history) } else { Ok(()) }
    }

    fn rewrite_file(&self, history: &mut LoadedHistory) -> io::Result<()> {
        let mut content = String::new();
        for entry in &history.entries {
            content.push_str(&entry_line(entry)?);
            content.push('\n');
        }
        write_atomic(&self.path, content.as_bytes())?;
        history.file_lines = history.entries.len();
        Ok(())
    }
}

enum State {
    Unloaded,
    Loaded(LoadedHistory),
}

struct LoadedHistory {
    entries: VecDeque<PromptHistoryEntry>,
    file_lines: usize,
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
        Err(error) => {
            warn!("Skipping malformed prompt history line: {error}");
            None
        }
    }
}

fn find_prompt_match(prompt: &str, query: &str) -> Option<(usize, usize)> {
    if query.chars().any(char::is_uppercase) {
        prompt.find(query).map(|start| (start, start + query.len()))
    } else {
        find_case_insensitive(prompt, query)
    }
}

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
            return orig_start.map(|start| (start, orig_idx));
        }
        lower_offset += ch.to_lowercase().map(char::len_utf8).sum::<usize>();
    }
    orig_start.map(|start| (start, prompt.len()))
}

fn keep_newest<T>(entries: Vec<T>) -> VecDeque<T> {
    let skip = entries.len().saturating_sub(PROMPT_HISTORY_MAX_ENTRIES);
    entries.into_iter().skip(skip).collect()
}

fn entry_line(entry: &PromptHistoryEntry) -> io::Result<String> {
    serde_json::to_string(entry)
        .map_err(|error| io::Error::other(format!("failed to serialize prompt history entry: {error}")))
}
