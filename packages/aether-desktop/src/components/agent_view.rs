//! Agent view component.
//!
//! Displays the chat interface for a single agent session.

use std::path::Path;
use std::sync::{Arc, LazyLock};

use agent_client_protocol::{
    ContentBlock, EmbeddedResource, EmbeddedResourceResource, ResourceLink, TextResourceContents,
};
use dioxus::prelude::*;
use regex::Regex;
use tokio::sync::Mutex;

/// Matches `@query` at start of string or after whitespace, capturing the query.
static FILE_MENTION_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|\s)@(\S*)$").unwrap());

use crate::file_search::{FileMatch, FileSearcher};
use crate::state::{
    now_iso, AgentStatus, AutocompleteMode, AutocompleteState, CommentKey, DiffComment, Message,
    MessageKind, Role, SlashCommand,
};
use crate::{with_agent_mut, AGENTS, FILE_SEARCHERS, HANDLES};

use super::command_dropdown::CommandDropdown;
use super::diff_view::DiffView;
use super::file_picker::{FilePicker, FilePill};
use super::message_bubble::MessageBubble;
use super::view_tabs::{AgentViewTab, ViewTabs};
use super::voice_input::VoiceInput;

#[component]
pub fn AgentView(agent_id: String) -> Element {
    let mut input_val = use_signal(String::new);
    let mut autocomplete_state = use_signal(AutocompleteState::default);
    let mut active_tab = use_signal(|| AgentViewTab::Chat);
    let agent_id_for_send = agent_id.clone();
    let agent_id_for_diff = agent_id.clone();

    // Pending file mentions (files selected via @)
    let mut pending_files: Signal<Vec<FileMatch>> = use_signal(Vec::new);

    // File search results
    let file_matches: Signal<Vec<FileMatch>> = use_signal(Vec::new);
    let mut files_loading = use_signal(|| false);

    let Some(agent_signal) = AGENTS.read().get(&agent_id) else {
        return rsx! {
            div {
                class: "flex-1 flex items-center justify-center text-gray-500",
                "Agent not found"
            }
        };
    };

    // Get the agent's cwd for file searching
    let agent_cwd = agent_signal.read().cwd.clone();

    // Get or create file searcher from global cache (shared across agent views with same cwd)
    let file_searcher: Arc<Mutex<FileSearcher>> =
        FILE_SEARCHERS.write().get_or_create(agent_cwd.clone());

    // Index files on first render (only if not already indexed)
    use_effect({
        let searcher = file_searcher.clone();
        move || {
            let searcher = searcher.clone();
            spawn(async move {
                let mut searcher = searcher.lock().await;
                if !searcher.is_indexed() {
                    files_loading.set(true);
                    if let Err(e) = searcher.index_files() {
                        tracing::warn!("Failed to index files: {}", e);
                    }
                    files_loading.set(false);
                }
            });
        }
    });

    let available_commands: Vec<SlashCommand> = agent_signal.read().available_commands.clone();

    let mut do_send = {
        let agent_id = agent_id_for_send.clone();
        move || {
            let content = input_val.read().clone();
            let files = pending_files.read().clone();

            if content.trim().is_empty() && files.is_empty() {
                return;
            }

            // Close autocomplete on send
            autocomplete_state.write().hide();

            // Add user message to state immediately (show original content, not file contents)
            let display_content = if files.is_empty() {
                content.clone()
            } else {
                let file_list: Vec<_> = files.iter().map(|f| format!("@{}", f.path)).collect();
                format!("{}\n\n{}", file_list.join(" "), content)
            };

            with_agent_mut(&agent_id, |agent| {
                agent.messages.push(Message {
                    id: uuid::Uuid::new_v4().to_string(),
                    role: Role::User,
                    content: display_content,
                    kind: MessageKind::Text,
                    timestamp: now_iso(),
                    is_streaming: false,
                });
                agent.status = AgentStatus::Running;
            });

            // Clear input immediately for responsiveness
            input_val.set(String::new());
            pending_files.write().clear();

            let agent_id = agent_id.clone();
            spawn(async move {
                let supports_embedded = HANDLES.read().supports_embedded_context(&agent_id);
                let mut prompt: Vec<ContentBlock> = Vec::new();

                for file in &files {
                    if supports_embedded {
                        match tokio::fs::read_to_string(&file.absolute_path).await {
                            Ok(file_content) => {
                                prompt.push(file_to_embedded_resource(file, &file_content));
                            }
                            Err(e) => {
                                tracing::warn!("Failed to read file {}: {}", file.path, e);
                            }
                        }
                    } else {
                        prompt.push(file_to_resource_link(file));
                    }
                }

                if !content.trim().is_empty() {
                    prompt.push(ContentBlock::from(content));
                }

                if let Err(e) = HANDLES.read().send_prompt(&agent_id, prompt) {
                    tracing::error!("Failed to send message: {}", e);
                    with_agent_mut(&agent_id, |agent| {
                        agent.status = AgentStatus::Error(e.to_string());
                    });
                }
            });
        }
    };

    // Handle command selection from dropdown
    let on_command_select = {
        let mut input_val = input_val;
        let mut autocomplete_state = autocomplete_state;
        move |cmd: SlashCommand| {
            // Replace input with "/{command} "
            input_val.set(format!("/{} ", cmd.name));
            autocomplete_state.write().hide();
        }
    };

    // Handle file selection from file picker
    let on_file_select = {
        let mut input_val = input_val;
        let mut autocomplete_state = autocomplete_state;
        let mut pending_files = pending_files;
        move |file: FileMatch| {
            add_file_to_pending(
                file,
                &mut pending_files,
                &mut input_val,
                &mut autocomplete_state,
            );
        }
    };

    // Handle input changes - detect "/" or "@" for autocomplete
    let on_input_change = {
        let mut autocomplete_state = autocomplete_state;
        let mut file_matches = file_matches;
        let file_searcher = file_searcher.clone();
        move |e: Event<FormData>| {
            let value = e.value();
            input_val.set(value.clone());

            // Check for slash commands (at start, no spaces)
            if value.starts_with('/') && !value.contains(' ') {
                let filter = value.trim_start_matches('/').to_string();
                *autocomplete_state.write() = AutocompleteState::slash_command(filter);
                return;
            }

            if let Some(caps) = FILE_MENTION_RE.captures(&value) {
                let query = caps.get(1).map_or("", |m| m.as_str()).to_string();

                *autocomplete_state.write() = AutocompleteState::file_mention(query.clone());

                let searcher = file_searcher.clone();
                spawn(async move {
                    let mut searcher = searcher.lock().await;
                    let matches = searcher.search(&query, 10);
                    file_matches.set(matches);
                });
                return;
            }

            // No trigger found, hide autocomplete
            autocomplete_state.write().hide();
        }
    };

    // Enhanced keyboard handling
    let on_keydown = {
        let mut do_send = do_send.clone();
        let mut autocomplete_state = autocomplete_state;
        let commands = available_commands.clone();
        let mut input_val = input_val;
        let mut pending_files = pending_files;

        move |e: KeyboardEvent| {
            let state = autocomplete_state.read().clone();

            if state.is_visible() {
                match state.mode {
                    AutocompleteMode::SlashCommand => {
                        // Slash command dropdown navigation
                        let filtered: Vec<&SlashCommand> = commands
                            .iter()
                            .filter(|cmd| {
                                state.filter_text.is_empty()
                                    || cmd
                                        .name
                                        .to_lowercase()
                                        .contains(&state.filter_text.to_lowercase())
                            })
                            .collect();

                        match e.key() {
                            Key::ArrowDown => {
                                e.prevent_default();
                                if !filtered.is_empty() {
                                    autocomplete_state.write().select_next(filtered.len() - 1);
                                }
                            }
                            Key::ArrowUp => {
                                e.prevent_default();
                                autocomplete_state.write().select_previous();
                            }
                            Key::Enter | Key::Tab => {
                                e.prevent_default();
                                if let Some(cmd) = filtered.get(state.selected_index) {
                                    input_val.set(format!("/{} ", cmd.name));
                                    autocomplete_state.write().hide();
                                }
                            }
                            Key::Escape => {
                                e.prevent_default();
                                autocomplete_state.write().hide();
                            }
                            _ => {}
                        }
                    }
                    AutocompleteMode::FileMention => {
                        // File picker navigation
                        let matches = file_matches.read();
                        let match_count = matches.len();

                        match e.key() {
                            Key::ArrowDown => {
                                e.prevent_default();
                                if match_count > 0 {
                                    autocomplete_state.write().select_next(match_count - 1);
                                }
                            }
                            Key::ArrowUp => {
                                e.prevent_default();
                                autocomplete_state.write().select_previous();
                            }
                            Key::Enter | Key::Tab => {
                                e.prevent_default();
                                if let Some(file) = matches.get(state.selected_index) {
                                    add_file_to_pending(
                                        file.clone(),
                                        &mut pending_files,
                                        &mut input_val,
                                        &mut autocomplete_state,
                                    );
                                }
                            }
                            Key::Escape => {
                                e.prevent_default();
                                autocomplete_state.write().hide();
                            }
                            _ => {}
                        }
                    }
                    AutocompleteMode::None => {}
                }
            } else {
                match e.key() {
                    Key::Enter if !e.modifiers().shift() => {
                        e.prevent_default();
                        do_send();
                    }
                    Key::Backspace => {
                        // If at start of input and we have pending files, remove the last one
                        let input = input_val.read().clone();
                        if input.is_empty() && !pending_files.read().is_empty() {
                            e.prevent_default();
                            pending_files.write().pop();
                        }
                    }
                    _ => {}
                }
            }
        }
    };

    let agent = agent_signal.read();
    tracing::debug!("AgentView rendering for agent: {}", agent.id);

    let is_running = matches!(agent.status, AgentStatus::Running);
    let status_text = match &agent.status {
        AgentStatus::Idle => "Idle",
        AgentStatus::Running => "Running...",
        AgentStatus::Error(_) => "Error",
    };
    let status_color = match &agent.status {
        AgentStatus::Idle => "bg-gray-600 text-gray-300",
        AgentStatus::Running => "bg-green-600/20 text-green-400 border border-green-600/30",
        AgentStatus::Error(_) => "bg-red-600/20 text-red-400 border border-red-600/30",
    };

    // Read autocomplete state for rendering
    let ac_state = autocomplete_state.read().clone();
    let current_file_matches = file_matches.read().clone();
    let current_pending_files = pending_files.read().clone();
    let is_files_loading = *files_loading.read();

    // Get diff state for this agent
    let diff_state = agent.diff_state.clone();

    rsx! {
        div {
            class: "flex-1 flex flex-col h-full bg-[#0f1116] overflow-hidden",

            // Header with agent name, status, and tabs
            div {
                class: "p-4 border-b border-[#2d313a] flex items-center justify-between",
                div {
                    class: "flex items-center gap-4",
                    div {
                        h2 { class: "text-lg font-semibold text-white tracking-tight", "{agent.name}" }
                        p { class: "text-sm text-gray-500 font-mono truncate max-w-xs", "{agent.config.command_line}" }
                    }
                    ViewTabs {
                        active: active_tab(),
                        on_change: move |tab| active_tab.set(tab),
                    }
                }
                span {
                    class: "px-3 py-1.5 rounded-full text-xs font-medium {status_color}",
                    "{status_text}"
                }
            }

            // Content area - either Chat or Diff view
            match active_tab() {
                AgentViewTab::Chat => rsx! {
                    // Message list
                    div {
                        class: "flex-1 overflow-y-auto px-3 py-2 space-y-1",
                        id: "message-list",

                        if agent.messages.is_empty() {
                            div {
                                class: "h-full flex items-center justify-center text-gray-500",
                                "Send a message to start the conversation"
                            }
                        }

                        for msg in agent.messages.iter() {
                            MessageBubble {
                                key: "{msg.id}",
                                message: msg.clone(),
                            }
                        }

                        // Scroll anchor
                        div { id: "message-end" }
                    }

                    // Input area with dropdown
                    div {
                        class: "p-4 border-t border-[#2d313a] bg-[#1a1d23]",

                        // File pills (pending file mentions)
                        if !current_pending_files.is_empty() {
                            div {
                                class: "flex flex-wrap gap-2 mb-3",
                                for file in current_pending_files.iter() {
                                    FilePill {
                                        key: "{file.path}",
                                        file: file.clone(),
                                        on_remove: {
                                            let path = file.path.clone();
                                            let mut pending_files = pending_files;
                                            move |_| {
                                                pending_files.write().retain(|f| f.path != path);
                                            }
                                        },
                                    }
                                }
                            }
                        }

                        // Relative container for dropdown positioning
                        div {
                            class: "relative",

                            // Autocomplete dropdown (positioned above input)
                            if ac_state.is_visible() {
                                match ac_state.mode {
                                    AutocompleteMode::SlashCommand => rsx! {
                                        CommandDropdown {
                                            commands: available_commands.clone(),
                                            filter: ac_state.filter_text.clone(),
                                            selected_index: ac_state.selected_index,
                                            on_select: on_command_select,
                                        }
                                    },
                                    AutocompleteMode::FileMention => rsx! {
                                        FilePicker {
                                            matches: current_file_matches.clone(),
                                            selected_index: ac_state.selected_index,
                                            loading: is_files_loading,
                                            on_select: on_file_select,
                                        }
                                    },
                                    AutocompleteMode::None => rsx! {},
                                }
                            }

                            div {
                                class: "flex gap-3",
                                textarea {
                                    class: "input-field flex-1 rounded-xl px-4 py-3 resize-none",
                                    value: "{input_val}",
                                    oninput: on_input_change,
                                    onkeydown: on_keydown,
                                    placeholder: "Type a message, / for commands, or @ to mention files...",
                                    disabled: is_running,
                                    rows: "2",
                                }
                                VoiceInput {
                                    on_transcription: move |text: String| {
                                        let current = input_val.read().clone();
                                        let new_value = if current.is_empty() {
                                            text
                                        } else {
                                            format!("{} {}", current, text)
                                        };
                                        input_val.set(new_value);
                                    },
                                    disabled: is_running,
                                }
                                button {
                                    class: "btn-primary px-6 py-3 rounded-xl font-semibold disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:scale-100",
                                    onclick: move |_| do_send(),
                                    disabled: is_running,
                                    if is_running {
                                        "Working..."
                                    } else {
                                        "Send"
                                    }
                                }
                            }
                        }
                    }
                },
                AgentViewTab::Diff => {
                    let agent_id = agent_id_for_diff.clone();

                    rsx! {
                        div {
                            class: "flex-1 overflow-hidden",
                            DiffView {
                                diff_state: diff_state,
                                on_file_select: {
                                    let agent_id = agent_id.clone();
                                    move |path: String| {
                                        with_agent_mut(&agent_id, |agent| {
                                            agent.diff_state.selected_file = Some(path);
                                        });
                                    }
                                },
                                on_add_comment: {
                                    let agent_id = agent_id.clone();
                                    move |comment: DiffComment| {
                                        with_agent_mut(&agent_id, |agent| {
                                            agent.diff_state.add_comment(comment);
                                        });
                                    }
                                },
                                on_edit_comment: {
                                    let agent_id = agent_id.clone();
                                    move |(key, new_content): (CommentKey, String)| {
                                        with_agent_mut(&agent_id, |agent| {
                                            agent.diff_state.update_comment(&key, new_content);
                                        });
                                    }
                                },
                                on_remove_comment: {
                                    let agent_id = agent_id.clone();
                                    move |key: CommentKey| {
                                        with_agent_mut(&agent_id, |agent| {
                                            agent.diff_state.remove_comment(&key);
                                        });
                                    }
                                },
                                on_clear_comments: {
                                    let agent_id = agent_id.clone();
                                    move |_| {
                                        with_agent_mut(&agent_id, |agent| {
                                            agent.diff_state.clear_comments();
                                        });
                                    }
                                },
                                on_send_comments: {
                                    let agent_id = agent_id.clone();
                                    let mut active_tab = active_tab;
                                    move |prompt: String| {
                                        // Add user message with the generated prompt
                                        with_agent_mut(&agent_id, |agent| {
                                            agent.messages.push(Message {
                                                id: uuid::Uuid::new_v4().to_string(),
                                                role: Role::User,
                                                content: prompt.clone(),
                                                kind: MessageKind::Text,
                                                timestamp: now_iso(),
                                                is_streaming: false,
                                            });
                                            agent.status = AgentStatus::Running;
                                            agent.diff_state.clear_comments();
                                        });

                                        if let Err(e) = HANDLES.read().send_prompt(&agent_id, vec![ContentBlock::from(prompt)]) {
                                            tracing::error!("Failed to send comment prompt: {}", e);
                                            with_agent_mut(&agent_id, |agent| {
                                                agent.status = AgentStatus::Error(e.to_string());
                                            });
                                        }

                                        // Switch to Chat tab to show the conversation
                                        active_tab.set(AgentViewTab::Chat);
                                    }
                                },
                            }
                        }
                    }
                },
            }
        }
    }
}

#[component]
pub fn EmptyState() -> Element {
    rsx! {
        div {
            class: "flex-1 flex flex-col items-center justify-center text-gray-500 bg-[#0f1116]",
            div {
                class: "w-20 h-20 mb-6 rounded-full bg-gradient-to-br from-blue-500/20 to-purple-500/20 flex items-center justify-center",
                svg {
                    xmlns: "http://www.w3.org/2000/svg",
                    width: "40",
                    height: "40",
                    view_box: "0 0 24 24",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "2",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                    class: "text-gray-400",
                    path {
                        d: "M12 5v14M5 12h14"
                    }
                }
            }
            p { class: "text-lg font-medium text-gray-400", "Create a new agent to get started" }
            p { class: "text-sm mt-2 text-gray-600", "Click the \"New Agent\" button in the sidebar" }
        }
    }
}

/// Helper to add a file to pending mentions and clear the @query from input.
fn add_file_to_pending(
    file: FileMatch,
    pending_files: &mut Signal<Vec<FileMatch>>,
    input_val: &mut Signal<String>,
    autocomplete_state: &mut Signal<AutocompleteState>,
) {
    // Add file if not already present
    {
        let mut files = pending_files.write();
        if !files.iter().any(|f| f.path == file.path) {
            files.push(file);
        }
    }

    // Remove the @query from input
    let current = input_val.read().clone();
    if let Some(at_pos) = current.rfind('@') {
        input_val.set(current[..at_pos].to_string());
    }

    autocomplete_state.write().hide();
}

/// Build a ContentBlock::Resource with embedded file content.
///
/// Used when the agent supports `embedded_context` capability.
fn file_to_embedded_resource(file: &FileMatch, content: &str) -> ContentBlock {
    ContentBlock::Resource(EmbeddedResource {
        annotations: None,
        resource: EmbeddedResourceResource::TextResourceContents(TextResourceContents {
            uri: format!("file://{}", file.absolute_path.display()),
            text: content.to_string(),
            mime_type: mime_from_path(&file.path),
            meta: None,
        }),
        meta: None,
    })
}

/// Build a ContentBlock::ResourceLink for a file reference.
///
/// Used when the agent does NOT support `embedded_context` - the agent will read the file itself.
fn file_to_resource_link(file: &FileMatch) -> ContentBlock {
    ContentBlock::ResourceLink(ResourceLink {
        uri: format!("file://{}", file.absolute_path.display()),
        name: file.path.clone(),
        size: Some(file.size as i64),
        mime_type: mime_from_path(&file.path),
        title: Some(file.path.clone()),
        description: None,
        annotations: None,
        meta: None,
    })
}

/// Infer MIME type from file extension.
fn mime_from_path(path: &str) -> Option<String> {
    let ext = Path::new(path).extension()?.to_str()?;
    let mime = match ext.to_lowercase().as_str() {
        "rs" => "text/x-rust",
        "py" => "text/x-python",
        "js" => "text/javascript",
        "ts" => "text/typescript",
        "tsx" => "text/typescript-jsx",
        "jsx" => "text/javascript-jsx",
        "json" => "application/json",
        "toml" => "text/x-toml",
        "yaml" | "yml" => "text/x-yaml",
        "md" => "text/markdown",
        "html" => "text/html",
        "css" => "text/css",
        "go" => "text/x-go",
        "java" => "text/x-java",
        "c" => "text/x-c",
        "cpp" | "cc" | "cxx" => "text/x-c++",
        "h" | "hpp" => "text/x-c-header",
        "sh" | "bash" => "text/x-shellscript",
        "sql" => "text/x-sql",
        "xml" => "text/xml",
        "txt" => "text/plain",
        _ => "text/plain",
    };
    Some(mime.to_string())
}
