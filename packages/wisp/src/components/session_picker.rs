use acp_utils::notifications::{SessionDisplayMeta, SessionPreviewResponse, SessionPreviewRole};
use agent_client_protocol::schema as acp;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tui::{
    BorderedTextField, Combobox, Component, Cursor, Event, Frame, FramePart, Line, MouseEventKind, PickerMessage,
    Searchable, Style, ViewContext, display_width_text, pad_text_to_width, truncate_text,
};

#[derive(Clone)]
pub struct SessionEntry(pub acp::SessionInfo);

impl Searchable for SessionEntry {
    fn search_text(&self) -> String {
        let SessionEntry(info) = self;
        let title = info.title.as_deref().unwrap_or("");
        let cwd = info.cwd.display();
        let meta = session_meta(info);
        format!("{title} {cwd} {} {}", meta.model.unwrap_or_default(), meta.selected_mode.unwrap_or_default())
    }
}

pub struct SessionPicker {
    combobox: Combobox<SessionEntry>,
    has_sessions: bool,
    preview_enabled: bool,
    previews: HashMap<String, PreviewState>,
}

pub enum SessionPickerMessage {
    Close,
    LoadSession { session_id: acp::SessionId, cwd: PathBuf },
    RequestPreview { session_id: acp::SessionId },
}

#[derive(Clone)]
enum PreviewState {
    Loading,
    Loaded(SessionPreviewResponse),
    Error(String),
}

impl SessionPicker {
    pub fn new(sessions: Vec<SessionEntry>, preview_enabled: bool) -> Self {
        let has_sessions = !sessions.is_empty();
        Self {
            combobox: Combobox::new(sessions).close_on_whitespace(false),
            has_sessions,
            preview_enabled,
            previews: HashMap::new(),
        }
    }

    pub fn initial_messages(&mut self) -> Vec<SessionPickerMessage> {
        self.ensure_selected_preview_requested()
            .map(|session_id| SessionPickerMessage::RequestPreview { session_id })
            .into_iter()
            .collect()
    }

    pub fn on_preview_loaded(&mut self, preview: SessionPreviewResponse) {
        self.previews.insert(preview.session_id.clone(), PreviewState::Loaded(preview));
    }

    pub fn on_preview_failed(&mut self, session_id: &str, error: String) {
        self.previews.insert(session_id.to_string(), PreviewState::Error(error));
    }

    fn selected_session_id(&self) -> Option<String> {
        self.combobox.selected().map(|entry| entry.0.session_id.0.to_string())
    }

    fn ensure_selected_preview_requested(&mut self) -> Option<acp::SessionId> {
        if !self.preview_enabled {
            return None;
        }
        let id = self.selected_session_id()?;
        if self.previews.contains_key(&id) {
            return None;
        }
        self.previews.insert(id.clone(), PreviewState::Loading);
        Some(acp::SessionId::new(id))
    }
}

impl Component for SessionPicker {
    type Message = SessionPickerMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        let before = self.selected_session_id();
        if let Event::Mouse(mouse) = event {
            match mouse.kind {
                MouseEventKind::ScrollUp => self.combobox.move_up(),
                MouseEventKind::ScrollDown => self.combobox.move_down(),
                _ => return Some(vec![]),
            }
            return Some(self.selection_messages(before.as_deref()));
        }

        let msgs = self.combobox.handle_picker_event(event)?;
        let mut mapped: Vec<_> = msgs
            .into_iter()
            .filter_map(|m| match m {
                PickerMessage::Close | PickerMessage::CloseAndPopChar => Some(SessionPickerMessage::Close),
                PickerMessage::Confirm(entry) => Some(SessionPickerMessage::LoadSession {
                    session_id: acp::SessionId::new(entry.0.session_id.0.to_string()),
                    cwd: entry.0.cwd,
                }),
                _ => None,
            })
            .collect();
        mapped.extend(self.selection_messages(before.as_deref()));
        Some(mapped)
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        if !self.has_sessions {
            return Frame::new(vec![Line::new(String::new()), Line::new("  No previous sessions found.")]);
        }

        let search = search_box_frame(self.combobox.query(), context);
        let now = Utc::now();
        let body_height = context.size.height.saturating_sub(u16::try_from(search.lines().len()).unwrap_or(0));
        let body_context = context.with_height(body_height);
        let list = self.render_list(&body_context, now).truncate_height(body_height);
        let body = if context.size.width >= WIDE_PREVIEW_THRESHOLD {
            let list_width = context.size.width / 2;
            let right_width = context.size.width.saturating_sub(list_width + 1);
            let right_frame =
                self.render_preview(&body_context.with_width(right_width), now).truncate_height(body_height);
            let sep_height = list.lines().len().max(right_frame.lines().len()).max(1);
            let left = FramePart::wrap(list, list_width);
            let sep_lines = vec![Line::with_style("│", Style::fg(context.theme.muted())); sep_height];
            let sep = FramePart::new(Frame::new(sep_lines), 1);
            let right = FramePart::wrap(right_frame, right_width);
            Frame::hstack([left, sep, right])
        } else {
            list
        };
        Frame::vstack([search, body])
    }
}

const SEARCH_BOX_MAX_WIDTH: usize = 56;
const SEARCH_BOX_INDENT: u16 = 2;
const SEARCH_LABEL: &str = "🔍 Search";
const SEARCH_PLACEHOLDER: &str = "type to search title or path";
const WIDE_PREVIEW_THRESHOLD: u16 = 96;
const MAX_TITLE_WIDTH: usize = 48;

impl SessionPicker {
    fn selection_messages(&mut self, before: Option<&str>) -> Vec<SessionPickerMessage> {
        if self.selected_session_id().as_deref() == before {
            return Vec::new();
        }
        self.ensure_selected_preview_requested()
            .map(|session_id| SessionPickerMessage::RequestPreview { session_id })
            .into_iter()
            .collect()
    }

    fn render_list(&mut self, context: &ViewContext, now: DateTime<Utc>) -> Frame {
        let mut list_lines = vec![Line::new(String::new())];

        if self.combobox.is_empty() {
            list_lines.push(Line::new("  (no matching sessions)"));
            return Frame::new(list_lines);
        }

        let max_title_width = self
            .combobox
            .matches()
            .iter()
            .map(|e| display_width_text(&display_title(&e.0)))
            .max()
            .unwrap_or(0)
            .min(MAX_TITLE_WIDTH);

        let item_lines = self.combobox.render_items(context, |SessionEntry(info), is_selected, ctx| {
            let display_title = display_title(info);
            let title = truncate_text(&display_title, max_title_width);
            let relative = info.updated_at.as_deref().map(|ts| format_short_datetime_at(ts, now)).unwrap_or_default();
            let meta = row_metadata(info, &relative);
            let padded_title = pad_text_to_width(&title, max_title_width);
            let line_text = if meta.is_empty() { padded_title.to_string() } else { format!("{padded_title}  {meta}") };
            let truncated = truncate_text(&line_text, ctx.size.width as usize);

            if is_selected {
                ctx.theme.selected_row_line(truncated)
            } else {
                let boundary = truncated.floor_char_boundary(padded_title.len().min(truncated.len()));
                let mut line = Line::new(&truncated[..boundary]);
                if truncated.len() > boundary {
                    line.push_with_style(&truncated[boundary..], Style::fg(ctx.theme.muted()));
                }
                line
            }
        });
        list_lines.extend(item_lines);
        Frame::new(list_lines)
    }

    fn render_preview(&self, context: &ViewContext, now: DateTime<Utc>) -> Frame {
        let mut lines =
            vec![Line::with_style(" Session preview", Style::fg(context.theme.info())), Line::new(String::new())];
        let Some(id) = self.selected_session_id() else {
            lines.push(Line::with_style(" No session selected", Style::fg(context.theme.muted())));
            return Frame::new(lines);
        };
        match self.previews.get(&id) {
            None => lines.push(Line::with_style(" Preview not requested", Style::fg(context.theme.muted()))),
            Some(PreviewState::Loading) => lines.push(Line::with_style(" Loading…", Style::fg(context.theme.muted()))),
            Some(PreviewState::Error(err)) => {
                lines.push(Line::with_style(format!(" Error: {err}"), Style::fg(context.theme.error())));
            }
            Some(PreviewState::Loaded(preview)) => push_preview_lines(&mut lines, preview, context, now),
        }
        Frame::new(lines)
    }
}

fn search_box_frame(query: &str, context: &ViewContext) -> Frame {
    let width = (context.size.width as usize).saturating_sub(usize::from(SEARCH_BOX_INDENT)).min(SEARCH_BOX_MAX_WIDTH);
    let input_width = width.saturating_sub(4);
    let visible_query = truncate_text(query, input_width);

    let mut field = BorderedTextField::new(SEARCH_LABEL, query.to_string()).placeholder(SEARCH_PLACEHOLDER);
    field.set_width(width);

    Frame::new(field.render_field(context, false))
        .with_cursor(Cursor::visible(1, 2 + display_width_text(&visible_query)))
        .indent(SEARCH_BOX_INDENT)
}

fn push_preview_lines(
    lines: &mut Vec<Line>,
    preview: &SessionPreviewResponse,
    context: &ViewContext,
    now: DateTime<Utc>,
) {
    let muted = Style::fg(context.theme.muted());
    lines.push(Line::with_style(format!(" Path: {}", display_path(&preview.cwd)), muted));
    lines.push(Line::with_style(format!(" Created: {}", format_short_datetime_at(&preview.created_at, now)), muted));
    let mode = preview.selected_mode.as_deref().unwrap_or("default");
    lines.push(Line::with_style(format!(" Model: {}  Mode: {mode}", preview.model), muted));
    if preview.tool_call_count > 0 {
        lines.push(Line::with_style(format!(" Tool calls: {}", preview.tool_call_count), muted));
    }
    lines.push(Line::new(String::new()));
    for turn in &preview.transcript {
        lines.push(preview_turn_line(turn.role, &turn.text, context));
    }
    if preview.truncated {
        lines.push(Line::with_style(" … preview truncated", muted));
    }
}

fn preview_turn_line(role: SessionPreviewRole, text: &str, context: &ViewContext) -> Line {
    let display_role = match role {
        SessionPreviewRole::User => "user",
        SessionPreviewRole::Assistant => "assistant",
    };
    let mut line = Line::new(" ");
    let label_style = if role == SessionPreviewRole::User {
        Style::fg(context.theme.accent()).bold()
    } else {
        Style::fg(context.theme.muted()).bold()
    };
    line.push_with_style(format!("{display_role}:"), label_style);
    line.push_text(format!(" {text}"));
    line
}

fn display_title(info: &acp::SessionInfo) -> String {
    info.title.clone().unwrap_or_else(|| {
        info.cwd.file_name().map_or_else(|| info.cwd.display().to_string(), |n| n.to_string_lossy().into_owned())
    })
}

fn row_metadata(info: &acp::SessionInfo, relative: &str) -> String {
    let meta = session_meta(info);
    [Some(display_path(&info.cwd)), non_empty(relative.to_string()), meta.model, meta.selected_mode]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

fn session_meta(info: &acp::SessionInfo) -> SessionDisplayMeta {
    SessionDisplayMeta::from_meta(info.meta.as_ref())
}

fn non_empty(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

fn display_path(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

pub fn format_short_datetime_at(iso: &str, now: DateTime<Utc>) -> String {
    let Ok(ts) = iso.parse::<DateTime<Utc>>() else {
        return iso.to_string();
    };
    if ts.format("%Y").to_string() == now.format("%Y").to_string() {
        ts.format("%b %-d %H:%M").to_string()
    } else {
        ts.format("%b %-d %Y %H:%M").to_string()
    }
}
