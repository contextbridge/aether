mod prompt_history;

use acp_utils::notifications::{
    PromptSearchParams, PromptSearchResponse, SessionPreviewParams, SessionPreviewResponse, SessionPreviewRole,
    SessionPreviewTurn,
};
use aether_core::events::{AgentEvent, MessageEvent, ToolEvent};
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use tracing::warn;
use utils::settings::aether_home;

use crate::error::{SessionLogError, SessionStoreError};
use crate::{SessionEvent, SessionLog, SessionLogEntry, SessionMeta, UserEvent};
use llm::ContentBlock;
use prompt_history::PromptHistoryIndex;

const PROMPT_HISTORY_FILE: &str = "prompt-history.jsonl";
const PREVIEW_TRANSCRIPT_TURNS: usize = 8;
const MAX_TITLE_LEN: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FileFingerprint {
    pub file_size: i64,
    pub file_mtime_ns: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredSessionFile {
    pub path: PathBuf,
    pub fingerprint: FileFingerprint,
}

impl FileFingerprint {
    pub fn read(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let metadata = fs::metadata(path)?;
        let modified = metadata.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default();
        Ok(Self {
            file_size: metadata.len().try_into().unwrap_or(i64::MAX),
            file_mtime_ns: modified.as_nanos().try_into().unwrap_or(i64::MAX),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummary {
    pub meta: SessionMeta,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanLimits {
    pub max_lines: usize,
    pub max_bytes: usize,
}

impl ScanLimits {
    pub const SUMMARY: Self = Self { max_lines: 64, max_bytes: 64 * 1024 };
    pub const PREVIEW: Self = Self { max_lines: 200, max_bytes: 128 * 1024 };
    pub const UNBOUNDED: Self = Self { max_lines: usize::MAX, max_bytes: usize::MAX };
}

pub fn discover_session_files(sessions_dir: impl AsRef<Path>) -> std::io::Result<Vec<DiscoveredSessionFile>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || path.file_name().and_then(|name| name.to_str()) == Some(PROMPT_HISTORY_FILE) {
            continue;
        }
        if path.extension().and_then(|extension| extension.to_str()) == Some("jsonl") {
            let path = path.canonicalize()?;
            files.push(DiscoveredSessionFile { path: path.clone(), fingerprint: FileFingerprint::read(&path)? });
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

pub struct SessionStore {
    dir: PathBuf,
    prompt_history: PromptHistoryIndex,
}

impl SessionStore {
    pub fn new() -> Result<Self, SessionStoreError> {
        let home = aether_home().ok_or(SessionStoreError::MissingAetherHome)?;
        Ok(Self::from_path(home.join("sessions")))
    }

    pub fn from_path(dir: PathBuf) -> Self {
        let prompt_history = PromptHistoryIndex::new(dir.join(PROMPT_HISTORY_FILE));
        Self { dir, prompt_history }
    }

    pub fn append_meta(&self, session_id: &str, meta: &SessionMeta) -> Result<(), SessionStoreError> {
        self.append_line(session_id, meta)
    }

    pub fn append_event(&self, session_id: &str, event: &SessionEvent) -> Result<(), SessionStoreError> {
        if !event.is_persisted() {
            return Ok(());
        }
        self.append_recorded_event(session_id, event)
    }

    pub fn append_recorded_event(&self, session_id: &str, event: &SessionEvent) -> Result<(), SessionStoreError> {
        self.append_line(session_id, event)?;
        if let Some(prompt) = event.user_content()
            && let Some(meta) = self.session_meta(session_id)
        {
            self.prompt_history.append_prompt(&meta, prompt).map_err(SessionStoreError::PromptHistory)?;
        }

        Ok(())
    }

    pub fn load(&self, session_id: &str) -> Option<(SessionMeta, Vec<SessionEvent>)> {
        let scan = read_bounded_session(&self.session_path(session_id), ScanLimits::UNBOUNDED).ok()?;
        Some((scan.meta, scan.events))
    }

    pub fn session_cwd(&self, session_id: &str) -> Option<PathBuf> {
        self.session_meta(session_id).map(|meta| meta.cwd)
    }

    /// Rewrites the session's recorded working directory to `new_cwd`, updating the
    /// metadata line and any matching prompt-history entries. Event lines are left
    /// unchanged.
    pub fn relocate(&self, session_id: &str, new_cwd: &Path) -> Result<(), SessionStoreError> {
        let path = self.session_path(session_id);
        let content = fs::read_to_string(&path)?;
        let mut lines = content.lines();
        let first = lines.next().ok_or(SessionStoreError::MissingMetadata)?;

        let mut meta: SessionMeta = serde_json::from_str(first.trim())
            .map_err(|source| SessionStoreError::InvalidMetadata { line_number: 1, source })?;
        meta.cwd = new_cwd.to_path_buf();
        let meta_line = serde_json::to_string(&meta)?;

        let tmp_path = path.with_extension("jsonl.tmp");
        {
            let mut file = File::create(&tmp_path)?;
            writeln!(file, "{meta_line}")?;
            for line in lines {
                writeln!(file, "{line}")?;
            }
        }
        fs::rename(tmp_path, &path)?;

        self.prompt_history.relocate_session(session_id, new_cwd).map_err(SessionStoreError::PromptHistory)
    }

    pub fn list(&self) -> Vec<SessionSummary> {
        let Ok(files) = discover_session_files(&self.dir) else {
            return Vec::new();
        };

        let mut summaries: Vec<SessionSummary> =
            files.into_iter().filter_map(|file| read_session_summary(&file.path).ok()).collect();

        summaries.sort_by(|a, b| b.meta.created_at.cmp(&a.meta.created_at));
        summaries
    }

    pub fn preview(&self, params: &SessionPreviewParams) -> Result<SessionPreviewResponse, SessionStoreError> {
        read_session_preview(&self.session_path(&params.session_id), ScanLimits::PREVIEW)
    }

    pub fn search_prompts(&self, params: &PromptSearchParams) -> Result<PromptSearchResponse, SessionStoreError> {
        self.prompt_history.search(params).map_err(SessionStoreError::PromptHistory)
    }

    fn session_meta(&self, session_id: &str) -> Option<SessionMeta> {
        SessionLog::open(self.session_path(session_id)).ok().map(|log| log.meta)
    }

    fn append_line<T: Serialize>(&self, session_id: &str, value: &T) -> Result<(), SessionStoreError> {
        fs::create_dir_all(&self.dir)?;
        let path = self.session_path(session_id);
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        let line = serde_json::to_string(value)?;
        writeln!(file, "{line}")?;
        Ok(())
    }

    fn session_path(&self, session_id: &str) -> PathBuf {
        self.dir.join(format!("{session_id}.jsonl"))
    }
}

struct SessionLogScan {
    meta: SessionMeta,
    events: Vec<SessionEvent>,
    truncated: bool,
}

fn read_session_summary(path: &Path) -> Result<SessionSummary, SessionStoreError> {
    let scan = read_bounded_session(path, ScanLimits::SUMMARY)?;
    let title = scan.events.iter().find_map(|event| match event {
        SessionEvent::User(UserEvent::Message { content }) => Some(extract_title(content)),
        _ => None,
    });
    Ok(SessionSummary { meta: scan.meta, title })
}

fn read_session_preview(path: &Path, limits: ScanLimits) -> Result<SessionPreviewResponse, SessionStoreError> {
    let scan = read_bounded_session(path, limits)?;
    let meta = scan.meta;
    let mut truncated = scan.truncated;
    let mut transcript = Vec::new();
    let mut tool_call_count = 0;

    for event in scan.events {
        match event {
            SessionEvent::User(UserEvent::Message { content }) => {
                let text = ContentBlock::join_text(&content);
                let text = if text.is_empty() { "[media prompt]".to_string() } else { text };
                if !push_preview_turn(&mut transcript, SessionPreviewRole::User, &text) {
                    truncated = true;
                }
            }
            SessionEvent::Agent(AgentEvent::Message(MessageEvent::Text { chunk, .. }))
                if !push_preview_turn(&mut transcript, SessionPreviewRole::Assistant, &chunk) =>
            {
                truncated = true;
            }
            SessionEvent::Agent(AgentEvent::Tool(ToolEvent::Call { .. })) => {
                tool_call_count += 1;
            }
            _ => {}
        }
    }

    Ok(SessionPreviewResponse {
        session_id: meta.session_id,
        cwd: meta.cwd,
        created_at: meta.created_at,
        model: meta.model,
        selected_mode: meta.selected_mode,
        transcript,
        tool_call_count,
        truncated,
    })
}

fn read_bounded_session(path: &Path, limits: ScanLimits) -> Result<SessionLogScan, SessionStoreError> {
    let mut log = SessionLog::open(path).map_err(session_log_error)?;
    let meta = log.meta.clone();
    let mut events = Vec::new();
    let mut truncated = false;
    let mut lines_since_meta = 0_usize;
    let mut bytes_since_meta = 0_usize;

    while let Some(entry) = log.next_entry().map_err(SessionStoreError::Io)? {
        let line = entry.line();
        if lines_since_meta >= limits.max_lines || bytes_since_meta.saturating_add(line.bytes_read) > limits.max_bytes {
            truncated = true;
            break;
        }
        lines_since_meta += 1;
        bytes_since_meta = bytes_since_meta.saturating_add(line.bytes_read);
        match entry {
            SessionLogEntry::Persisted { event, .. } => events.push(*event),
            SessionLogEntry::Transient { .. } => {}
            SessionLogEntry::Malformed { error, .. } => warn!("Skipping malformed session log line: {error}"),
        }
    }

    Ok(SessionLogScan { meta, events, truncated })
}

fn session_log_error(error: SessionLogError) -> SessionStoreError {
    match error {
        SessionLogError::Io(error) => SessionStoreError::Io(error),
        SessionLogError::MissingMetadata => SessionStoreError::MissingMetadata,
        SessionLogError::InvalidMetadata { line_number, source } => {
            SessionStoreError::InvalidMetadata { line_number, source }
        }
    }
}

fn push_preview_turn(transcript: &mut Vec<SessionPreviewTurn>, role: SessionPreviewRole, text: &str) -> bool {
    let text = text.lines().next().unwrap_or(text).trim();
    if text.is_empty() {
        return true;
    }
    if transcript.len() >= PREVIEW_TRANSCRIPT_TURNS {
        return false;
    }
    transcript.push(SessionPreviewTurn { role, text: truncate_for_preview(text) });
    true
}

fn truncate_for_preview(text: &str) -> String {
    if text.len() <= MAX_TITLE_LEN {
        text.to_string()
    } else {
        let end = text.floor_char_boundary(MAX_TITLE_LEN);
        format!("{}…", &text[..end])
    }
}

fn extract_title(content: &[ContentBlock]) -> String {
    let first_line =
        ContentBlock::first_text(content).and_then(|text| text.lines().next()).unwrap_or("Media prompt").trim();
    if first_line.len() > MAX_TITLE_LEN {
        let end = first_line.floor_char_boundary(MAX_TITLE_LEN);
        format!("{}…", &first_line[..end])
    } else {
        first_line.to_string()
    }
}
