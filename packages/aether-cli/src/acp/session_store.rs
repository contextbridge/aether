use super::prompt_history_index::PromptHistoryIndex;
use acp_utils::notifications::{
    PromptSearchParams, PromptSearchResponse, SessionPreviewParams, SessionPreviewResponse, SessionPreviewRole,
    SessionPreviewTurn,
};
use aether_core::context::ext::{SessionEvent, UserEvent};
use aether_core::events::AgentMessage;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use tracing::warn;

const PROMPT_HISTORY_FILE: &str = "prompt-history.jsonl";
const PREVIEW_TRANSCRIPT_TURNS: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub session_id: String,
    pub cwd: PathBuf,
    pub model: String,
    #[serde(default)]
    pub selected_mode: Option<String>,
    pub created_at: String,
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

pub struct SessionStore {
    dir: PathBuf,
    prompt_history: PromptHistoryIndex,
}

impl SessionStore {
    pub fn new() -> io::Result<Self> {
        let home =
            dirs::home_dir().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Home directory not found"))?;
        Ok(Self::from_path(home.join(".aether/sessions")))
    }

    pub(crate) fn from_path(dir: PathBuf) -> Self {
        let prompt_history = PromptHistoryIndex::new(dir.join(PROMPT_HISTORY_FILE));
        Self { dir, prompt_history }
    }

    pub fn append_meta(&self, session_id: &str, meta: &SessionMeta) -> io::Result<()> {
        self.append_line(session_id, meta)
    }

    pub fn append_event(&self, session_id: &str, event: &SessionEvent) -> io::Result<()> {
        if is_streaming_event(event) {
            return Ok(());
        }
        self.append_line(session_id, event)?;
        if let Some(prompt) = user_prompt_text_from_event(event)
            && let Some(meta) = self.session_meta(session_id)
        {
            self.prompt_history.append_prompt(&meta, prompt)?;
        }
        Ok(())
    }

    pub fn load(&self, session_id: &str) -> Option<(SessionMeta, Vec<SessionEvent>)> {
        let scan = SessionLogReader::open(&self.session_path(session_id)).ok()?.scan(ScanLimits::UNBOUNDED).ok()?;
        Some((scan.meta, scan.events))
    }

    /// Rewrite the stored meta line so the session is re-homed to `cwd`,
    /// preserving every event line byte-for-byte. Only safe while no live
    /// actor is appending to this session's log.
    pub fn update_meta_cwd(&self, session_id: &str, cwd: &Path) -> io::Result<()> {
        let mut log = SessionLogReader::open(&self.session_path(session_id))?;
        if log.meta.cwd == cwd {
            return Ok(());
        }
        log.meta.cwd = cwd.to_path_buf();

        let temp_path = temp_session_path(&self.dir, session_id);
        let result = write_rewritten_log(&temp_path, &log.meta, &mut log.reader)
            .and_then(|()| fs::rename(&temp_path, self.session_path(session_id)));
        if result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        result
    }

    pub fn list(&self) -> Vec<SessionSummary> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };

        let mut summaries: Vec<SessionSummary> = entries
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl")
                    || self.prompt_history.is_index_path(&path)
                {
                    return None;
                }
                read_session_summary(&path).ok()
            })
            .collect();

        summaries.sort_by(|a, b| b.meta.created_at.cmp(&a.meta.created_at));
        summaries
    }

    pub fn preview(&self, params: &SessionPreviewParams) -> io::Result<SessionPreviewResponse> {
        read_session_preview(&self.session_path(&params.session_id), ScanLimits::PREVIEW)
    }

    pub fn search_prompts(&self, params: &PromptSearchParams) -> io::Result<PromptSearchResponse> {
        self.prompt_history.search(params)
    }

    fn session_meta(&self, session_id: &str) -> Option<SessionMeta> {
        let file = File::open(self.session_path(session_id)).ok()?;
        let mut reader = BufReader::new(file);
        let mut first_line = String::new();
        reader.read_line(&mut first_line).ok()?;
        serde_json::from_str(first_line.trim()).ok()
    }

    fn append_line<T: Serialize>(&self, session_id: &str, value: &T) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.session_path(session_id);
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        let line = serde_json::to_string(value)
            .map_err(|e| io::Error::other(format!("Failed to serialize log entry: {e}")))?;
        writeln!(file, "{line}")?;
        Ok(())
    }

    fn session_path(&self, session_id: &str) -> PathBuf {
        self.dir.join(format!("{session_id}.jsonl"))
    }
}

fn read_session_summary(path: &Path) -> io::Result<SessionSummary> {
    let scan = read_bounded_session(path, ScanLimits::SUMMARY)?;
    let title = scan.events.iter().find_map(|event| match event {
        SessionEvent::User(UserEvent::Message { content }) => Some(extract_title(content)),
        _ => None,
    });
    Ok(SessionSummary { meta: scan.meta, title })
}

fn read_session_preview(path: &Path, limits: ScanLimits) -> io::Result<SessionPreviewResponse> {
    let scan = read_bounded_session(path, limits)?;
    let meta = scan.meta;
    let events = scan.events;
    let mut truncated = scan.truncated;
    let mut transcript = Vec::new();
    let mut tool_call_count = 0;

    for event in events {
        match event {
            SessionEvent::User(UserEvent::Message { content }) => {
                let text = llm::ContentBlock::join_text(&content);
                let text = if text.is_empty() { "[media prompt]".to_string() } else { text };
                if !push_preview_turn(&mut transcript, SessionPreviewRole::User, &text) {
                    truncated = true;
                }
            }
            SessionEvent::Agent(AgentMessage::Text { chunk, .. })
                if !push_preview_turn(&mut transcript, SessionPreviewRole::Assistant, &chunk) =>
            {
                truncated = true;
            }
            SessionEvent::Agent(AgentMessage::ToolCall { .. }) => {
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

struct SessionLogReader {
    reader: BufReader<File>,
    meta: SessionMeta,
    bytes_read: usize,
}

struct SessionLogScan {
    meta: SessionMeta,
    events: Vec<SessionEvent>,
    truncated: bool,
}

impl SessionLogReader {
    fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        let bytes_read = reader.read_line(&mut line)?;
        if bytes_read == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "session file is empty"));
        }
        let meta = serde_json::from_str::<SessionMeta>(line.trim())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("invalid session metadata: {e}")))?;
        Ok(Self { reader, meta, bytes_read })
    }

    fn scan(mut self, limits: ScanLimits) -> io::Result<SessionLogScan> {
        let mut line = String::new();
        let mut lines_read = 0;
        let mut events = Vec::new();
        let mut truncated = false;

        loop {
            if lines_read >= limits.max_lines || self.bytes_read >= limits.max_bytes {
                truncated = true;
                break;
            }
            line.clear();
            let read = self.reader.read_line(&mut line)?;
            if read == 0 {
                break;
            }
            self.bytes_read = self.bytes_read.saturating_add(read);
            lines_read += 1;
            if self.bytes_read > limits.max_bytes {
                truncated = true;
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<SessionEvent>(trimmed) {
                Ok(event) => events.push(event),
                Err(e) => warn!("Skipping malformed session log line: {e}"),
            }
        }

        Ok(SessionLogScan { meta: self.meta, events, truncated })
    }
}

fn read_bounded_session(path: &Path, limits: ScanLimits) -> io::Result<SessionLogScan> {
    SessionLogReader::open(path)?.scan(limits)
}

fn write_rewritten_log(temp_path: &Path, meta: &SessionMeta, rest: &mut BufReader<File>) -> io::Result<()> {
    let mut file = File::create(temp_path)?;
    let meta_line =
        serde_json::to_string(meta).map_err(|e| io::Error::other(format!("Failed to serialize session meta: {e}")))?;
    writeln!(file, "{meta_line}")?;
    io::copy(rest, &mut file)?;
    file.sync_all()
}

fn temp_session_path(dir: &Path, session_id: &str) -> PathBuf {
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |duration| duration.as_nanos());
    dir.join(format!("{session_id}.jsonl.tmp.{}.{nanos}", std::process::id()))
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

const MAX_TITLE_LEN: usize = 80;

fn extract_title(content: &[llm::ContentBlock]) -> String {
    let first_line =
        llm::ContentBlock::first_text(content).and_then(|text| text.lines().next()).unwrap_or("Media prompt").trim();
    if first_line.len() > MAX_TITLE_LEN {
        let end = first_line.floor_char_boundary(MAX_TITLE_LEN);
        format!("{}…", &first_line[..end])
    } else {
        first_line.to_string()
    }
}

fn user_prompt_text_from_event(event: &SessionEvent) -> Option<String> {
    match event {
        SessionEvent::User(UserEvent::Message { content }) => {
            let joined = llm::ContentBlock::join_text(content);
            if joined.is_empty() { None } else { Some(joined) }
        }
        _ => None,
    }
}

pub(crate) fn is_streaming_event(event: &SessionEvent) -> bool {
    matches!(
        event,
        SessionEvent::Agent(
            AgentMessage::Text { is_complete: false, .. } | AgentMessage::Thought { is_complete: false, .. }
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::context::ext::{SessionControlEvent, UserEvent};
    use llm::ToolCallResult;

    fn meta(id: &str, created_at: &str, mode: Option<&str>) -> SessionMeta {
        SessionMeta {
            session_id: id.to_string(),
            cwd: PathBuf::from("/tmp"),
            model: "test-model".to_string(),
            selected_mode: mode.map(str::to_string),
            created_at: created_at.to_string(),
        }
    }

    fn default_meta() -> SessionMeta {
        meta("s1", "2026-01-01T00:00:00Z", Some("planner"))
    }

    fn user_msg(content: &str) -> SessionEvent {
        SessionEvent::User(UserEvent::Message { content: vec![llm::ContentBlock::text(content)] })
    }

    fn agent_text(msg_id: &str, chunk: &str, complete: bool) -> SessionEvent {
        SessionEvent::Agent(AgentMessage::Text {
            message_id: msg_id.to_string(),
            chunk: chunk.to_string(),
            is_complete: complete,
            model_name: "test".to_string(),
        })
    }

    fn switch_agent(from: Option<&str>, to: Option<&str>) -> SessionEvent {
        SessionEvent::Control(SessionControlEvent::AgentSwitched {
            from: from.map(str::to_string),
            to: to.map(str::to_string),
        })
    }

    fn temp_store() -> (tempfile::TempDir, SessionStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::from_path(dir.path().to_path_buf());
        (dir, store)
    }

    fn listed_title(content: Option<&str>) -> Option<String> {
        let (_dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        if let Some(c) = content {
            store.append_event("s1", &user_msg(c)).unwrap();
        }
        store.list().into_iter().next().unwrap().title
    }

    #[test]
    fn append_meta_persists_selected_mode_field() {
        let (dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        let raw = std::fs::read_to_string(dir.path().join("s1.jsonl")).unwrap();
        assert!(raw.contains("\"selectedMode\""), "missing selectedMode: {raw}");
    }

    #[test]
    fn append_and_load_roundtrip() {
        let (_dir, store) = temp_store();
        let m = default_meta();
        let user = user_msg("Hello");
        let agent = agent_text("msg_1", "Hi there!", true);

        store.append_meta("s1", &m).unwrap();
        store.append_event("s1", &user).unwrap();
        store.append_event("s1", &agent).unwrap();

        let (loaded, events) = store.load("s1").unwrap();
        assert_eq!(loaded, m);
        assert_eq!(events, vec![user, agent]);
    }

    #[test]
    fn control_events_roundtrip() {
        let (_dir, store) = temp_store();
        let m = default_meta();
        let control = switch_agent(Some("Planner"), Some("Coder"));
        store.append_meta("s1", &m).unwrap();
        store.append_event("s1", &control).unwrap();

        let (loaded, events) = store.load("s1").unwrap();
        assert_eq!(loaded, m);
        assert_eq!(events, vec![control]);
    }

    #[test]
    fn prompt_history_ignores_control_events() {
        let (_dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        store.append_event("s1", &switch_agent(Some("Planner"), Some("Coder"))).unwrap();

        let response = store.search_prompts(&PromptSearchParams { query: "Coder".to_string(), limit: None }).unwrap();
        assert!(response.results.is_empty());
    }

    #[test]
    fn load_skips_malformed_trailing_line() {
        let (dir, store) = temp_store();
        let m = default_meta();
        let mut file = File::create(dir.path().join("s2.jsonl")).unwrap();
        writeln!(file, "{}", serde_json::to_string(&m).unwrap()).unwrap();
        writeln!(file, "{{truncated garbage").unwrap();

        let (loaded, events) = store.load("s2").unwrap();
        assert_eq!(loaded, m);
        assert!(events.is_empty());
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let (_dir, store) = temp_store();
        assert!(store.load("nonexistent").is_none());
    }

    #[test]
    fn update_meta_cwd_rewrites_meta_and_preserves_events() {
        let (_dir, store) = temp_store();
        let events = vec![user_msg("Hello"), agent_text("msg_1", "Hi!", true)];
        store.append_meta("s1", &default_meta()).unwrap();
        for event in &events {
            store.append_event("s1", event).unwrap();
        }

        store.update_meta_cwd("s1", Path::new("/new/workspace")).unwrap();

        let (meta, loaded_events) = store.load("s1").unwrap();
        assert_eq!(meta.cwd, PathBuf::from("/new/workspace"));
        assert_eq!(meta, SessionMeta { cwd: PathBuf::from("/new/workspace"), ..default_meta() });
        assert_eq!(loaded_events, events);
    }

    #[test]
    fn update_meta_cwd_same_cwd_leaves_file_untouched() {
        let (dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        store.append_event("s1", &user_msg("Hello")).unwrap();
        let path = dir.path().join("s1.jsonl");
        let before = std::fs::read(&path).unwrap();

        store.update_meta_cwd("s1", &default_meta().cwd).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn update_meta_cwd_missing_session_returns_not_found() {
        let (_dir, store) = temp_store();
        let err = store.update_meta_cwd("missing", Path::new("/new")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn update_meta_cwd_preserves_malformed_trailing_bytes_verbatim() {
        let (dir, store) = temp_store();
        let path = dir.path().join("s1.jsonl");
        let trailing = format!("{}\n{{truncated garbage", serde_json::to_string(&user_msg("Hello")).unwrap());
        let mut file = File::create(&path).unwrap();
        writeln!(file, "{}", serde_json::to_string(&default_meta()).unwrap()).unwrap();
        write!(file, "{trailing}").unwrap();
        drop(file);

        store.update_meta_cwd("s1", Path::new("/new/workspace")).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let (meta_line, rest) = raw.split_once('\n').unwrap();
        assert_eq!(serde_json::from_str::<SessionMeta>(meta_line).unwrap().cwd, PathBuf::from("/new/workspace"));
        assert_eq!(rest, trailing);
    }

    #[test]
    fn update_meta_cwd_leaves_no_temp_files_and_list_still_finds_session() {
        let (dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        store.update_meta_cwd("s1", Path::new("/new/workspace")).unwrap();

        let temp_count = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp."))
            .count();
        assert_eq!(temp_count, 0);

        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].meta.cwd, PathBuf::from("/new/workspace"));
    }

    #[test]
    fn append_drops_streaming_chunks_and_persists_everything_else() {
        let (_dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();

        let dropped = [
            agent_text("m", "partial", false),
            SessionEvent::Agent(AgentMessage::Thought {
                message_id: "m".to_string(),
                chunk: "thinking".to_string(),
                is_complete: false,
                model_name: "test".to_string(),
            }),
        ];
        let kept = vec![
            agent_text("m", "full", true),
            SessionEvent::Agent(AgentMessage::Error { message: "oops".to_string() }),
            SessionEvent::Agent(AgentMessage::Done),
            SessionEvent::Agent(AgentMessage::ToolResult {
                result: ToolCallResult {
                    id: "1".to_string(),
                    name: "t".to_string(),
                    arguments: "{}".to_string(),
                    result: "ok".to_string(),
                },
                result_meta: None,
                model_name: "test".to_string(),
            }),
            SessionEvent::Agent(AgentMessage::ToolCallUpdate {
                tool_call_id: "1".to_string(),
                chunk: r#"{"filePath":"Cargo.toml"}"#.to_string(),
                model_name: "test".to_string(),
            }),
        ];

        for e in &dropped {
            store.append_event("s1", e).unwrap();
        }
        for e in &kept {
            store.append_event("s1", e).unwrap();
        }

        let (_, events) = store.load("s1").unwrap();
        assert_eq!(events, kept);
    }

    #[test]
    fn list_empty_and_nonexistent_dirs_return_empty() {
        let (_dir, store) = temp_store();
        assert!(store.list().is_empty());

        let missing = SessionStore::from_path(PathBuf::from("/nonexistent/path"));
        assert!(missing.list().is_empty());
    }

    #[test]
    fn list_returns_sessions_sorted_by_created_at_descending() {
        let (_dir, store) = temp_store();
        let old = meta("s-old", "2026-01-01T00:00:00Z", None);
        let new = meta("s-new", "2026-02-01T00:00:00Z", None);
        store.append_meta("s-old", &old).unwrap();
        store.append_meta("s-new", &new).unwrap();

        let ids: Vec<_> = store.list().iter().map(|s| s.meta.session_id.clone()).collect();
        assert_eq!(ids, vec!["s-new", "s-old"]);
    }

    #[test]
    fn list_skips_malformed_files() {
        let (dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        std::fs::write(dir.path().join("bad.jsonl"), "not valid json\n").unwrap();
        std::fs::write(dir.path().join("readme.txt"), "ignore me").unwrap();

        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].meta.session_id, "s1");
    }

    #[test]
    fn list_ignores_prompt_history_file() {
        let (dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        store.append_event("s1", &user_msg("hello world")).unwrap();

        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].meta.session_id, "s1");
        assert!(dir.path().join(PROMPT_HISTORY_FILE).exists());
    }

    #[test]
    fn prompt_history_searches_recent_user_prompts() {
        let (_dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        store.append_event("s1", &user_msg("hello world")).unwrap();
        store.append_event("s1", &agent_text("msg", "hello from agent", true)).unwrap();

        let response = store.search_prompts(&PromptSearchParams { query: "hello".to_string(), limit: None }).unwrap();
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].prompt, "hello world");
        assert_eq!(
            &response.results[0].prompt[response.results[0].match_start..response.results[0].match_end],
            "hello"
        );
    }

    #[test]
    fn prompt_history_keeps_only_last_entries() {
        let (_dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        for i in 0..105 {
            store.append_event("s1", &user_msg(&format!("prompt {i}"))).unwrap();
        }

        let old = store.search_prompts(&PromptSearchParams { query: "prompt 0".to_string(), limit: None }).unwrap();
        assert!(old.results.is_empty());

        let newest =
            store.search_prompts(&PromptSearchParams { query: "prompt 104".to_string(), limit: None }).unwrap();
        assert_eq!(newest.results.len(), 1);
    }

    #[test]
    fn prompt_history_matching_is_literal_smart_case_and_unicode_safe() {
        let (_dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        store.append_event("s1", &user_msg("hello world")).unwrap();
        store.append_event("s1", &user_msg("HELLO world")).unwrap();
        store.append_event("s1", &user_msg("hello.world")).unwrap();
        store.append_event("s1", &user_msg("café hello")).unwrap();

        let literal =
            store.search_prompts(&PromptSearchParams { query: "hello.world".to_string(), limit: None }).unwrap();
        assert_eq!(literal.results.len(), 1);
        assert_eq!(literal.results[0].prompt, "hello.world");

        let lower = store.search_prompts(&PromptSearchParams { query: "hello".to_string(), limit: None }).unwrap();
        assert!(lower.results.iter().any(|hit| hit.prompt == "hello world"));
        assert!(lower.results.iter().any(|hit| hit.prompt == "HELLO world"));

        let upper = store.search_prompts(&PromptSearchParams { query: "Hello".to_string(), limit: None }).unwrap();
        assert!(upper.results.is_empty());

        let unicode = store.search_prompts(&PromptSearchParams { query: "fé".to_string(), limit: None }).unwrap();
        assert_eq!(unicode.results.len(), 1);
        let hit = &unicode.results[0];
        assert_eq!(&hit.prompt[hit.match_start..hit.match_end], "fé");
    }

    #[test]
    fn list_finds_first_user_prompt_after_non_user_events() {
        let (_dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        store.append_event("s1", &switch_agent(Some("Planner"), Some("Coder"))).unwrap();
        store.append_event("s1", &agent_text("msg", "setup", true)).unwrap();
        store.append_event("s1", &user_msg("First useful prompt")).unwrap();

        let title = store.list().into_iter().next().unwrap().title;
        assert_eq!(title.as_deref(), Some("First useful prompt"));
    }

    #[test]
    fn list_ignores_malformed_content_after_summary_limit() {
        let (dir, store) = temp_store();
        let m = default_meta();
        let mut file = File::create(dir.path().join("s1.jsonl")).unwrap();
        writeln!(file, "{}", serde_json::to_string(&m).unwrap()).unwrap();
        writeln!(file, "{}", serde_json::to_string(&user_msg("bounded prompt")).unwrap()).unwrap();
        for _ in 0..ScanLimits::SUMMARY.max_lines {
            writeln!(file).unwrap();
        }
        writeln!(file, "{{malformed after limit").unwrap();

        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title.as_deref(), Some("bounded prompt"));
    }

    #[test]
    fn preview_returns_transcript_metadata_and_truncation() {
        let (_dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        store.append_event("s1", &user_msg("Preview this session")).unwrap();
        store.append_event("s1", &agent_text("msg", "Assistant reply", true)).unwrap();

        for i in 0..ScanLimits::PREVIEW.max_lines {
            store.append_event("s1", &user_msg(&format!("extra {i}"))).unwrap();
        }

        let preview = store.preview(&SessionPreviewParams { session_id: "s1".to_string() }).unwrap();

        assert_eq!(preview.model, "test-model");
        assert_eq!(preview.selected_mode.as_deref(), Some("planner"));
        assert_eq!(preview.transcript[0].role, SessionPreviewRole::User);
        assert!(preview.truncated);
    }

    #[test]
    fn preview_unknown_session_returns_not_found() {
        let (_dir, store) = temp_store();
        let err = store.preview(&SessionPreviewParams { session_id: "missing".to_string() }).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn list_title_extraction() {
        let cases: &[(&str, Option<&str>)] =
            &[("Fix the login bug", Some("Fix the login bug")), ("First line\nSecond\nThird", Some("First line"))];
        for (input, expected) in cases {
            assert_eq!(listed_title(Some(input)).as_deref(), *expected, "input: {input}");
        }
    }

    #[test]
    fn list_returns_none_title_when_no_user_message() {
        assert_eq!(listed_title(None), None);
    }

    #[test]
    fn list_truncates_long_title() {
        let title = listed_title(Some(&"a".repeat(120))).unwrap();
        assert!(title.len() <= 84);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn list_uses_media_prompt_title_when_no_text_blocks_exist() {
        let (_dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        store
            .append_event(
                "s1",
                &SessionEvent::User(UserEvent::Message {
                    content: vec![llm::ContentBlock::Image {
                        data: "aW1n".to_string(),
                        mime_type: "image/png".to_string(),
                    }],
                }),
            )
            .unwrap();

        let title = store.list().into_iter().next().unwrap().title;
        assert_eq!(title.as_deref(), Some("Media prompt"));
    }
}
