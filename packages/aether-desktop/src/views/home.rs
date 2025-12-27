//! Home view for the desktop app.
//!
//! This is the main view that displays the agent sidebar and chat interface.

use crate::acp_agent::{ActorError, AgentEvent, AgentHandle};
use crate::components::{AgentView, EmptyState, NewAgentForm, SettingsEditor, Sidebar};
use crate::settings::Settings;
use crate::state::{
    now_iso, AgentConfig, AgentHandles, AgentSession, AgentStatus, Message, MessageKind, Role,
    SlashCommand, ToolCallStatus,
};
use crate::{EventChannel, AGENTS, HANDLES};
use agent_client_protocol::{RequestPermissionOutcome, RequestPermissionResponse};
use dioxus::prelude::*;
use std::env::current_dir;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Represents which view to show in the right pane
#[derive(Clone, PartialEq)]
enum RightPaneView {
    Empty,
    Agent(String), // Agent UUID
    NewAgentForm,
    Settings,
}

#[component]
pub fn Home() -> Element {
    // AGENTS and HANDLES are global signals defined in main.rs
    let event_channel: EventChannel = use_context();
    let event_tx = event_channel.0;

    let mut right_pane: Signal<RightPaneView> = use_signal(|| RightPaneView::Empty);
    let mut error_message: Signal<Option<String>> = use_signal(|| None);

    let settings: Signal<Settings> = use_signal(Settings::load_or_default);
    use_context_provider(|| settings);

    let on_create_agent = move |(config, initial_message): (AgentConfig, String)| {
        let event_tx = event_tx.clone();
        spawn(async move {
            match create_agent(&AGENTS, &HANDLES, event_tx, config, initial_message).await {
                Ok(agent_id) => {
                    right_pane.set(RightPaneView::Agent(agent_id));
                    error_message.set(None);
                }
                Err(e) => {
                    error_message.set(Some(e.to_string()));
                }
            }
        });
    };

    // Derive selected_id for sidebar highlighting
    let selected_id = match &*right_pane.read() {
        RightPaneView::Agent(id) => Some(id.clone()),
        _ => None,
    };

    rsx! {
        div {
            class: "flex h-screen bg-[#0f1116] text-white font-sans",

            Sidebar {
                agents: AGENTS.signal(),
                selected_id: selected_id,
                on_new_agent: move |_| right_pane.set(RightPaneView::NewAgentForm),
                on_select_agent: move |id| right_pane.set(RightPaneView::Agent(id)),
                on_settings: move |_| right_pane.set(RightPaneView::Settings),
            }

            match &*right_pane.read() {
                RightPaneView::Empty => rsx! { EmptyState {} },
                RightPaneView::Agent(id) => rsx! { AgentView { agent_id: id.clone() } },
                RightPaneView::NewAgentForm => rsx! {
                    NewAgentForm {
                        on_create: on_create_agent,
                        on_cancel: move |_| right_pane.set(RightPaneView::Empty),
                    }
                },
                RightPaneView::Settings => rsx! {
                    SettingsEditor {
                        on_close: move |_| right_pane.set(RightPaneView::Empty),
                    }
                },
            }

            // Error toast
            if let Some(err) = error_message.read().as_ref() {
                div {
                    class: "fixed bottom-4 right-4 bg-[#1a1d23] border border-red-500/30 text-white px-5 py-4 rounded-xl shadow-2xl max-w-md animate-fade-in z-50",
                    div { class: "flex items-center gap-3",
                        span { class: "font-semibold text-red-400", "Error" }
                        button {
                            class: "ml-auto text-gray-400 hover:text-white transition-colors p-1 rounded hover:bg-white/10",
                            onclick: move |_| error_message.set(None),
                            "✕"
                        }
                    }
                    p { class: "mt-2 text-sm text-gray-200 leading-relaxed", "{err}" }
                }
            }
        }
    }
}

/// Single UI consumer that processes events from all agents.
///
/// This is spawned at the App level using GlobalSignals to avoid
/// Dioxus CopyValue scope warnings.
pub async fn run_ui_consumer(
    mut ui_rx: mpsc::UnboundedReceiver<AgentEvent>,
    agents: &GlobalSignal<Vec<AgentSession>>,
    handles: &GlobalSignal<AgentHandles>,
) {
    info!("UI consumer started");

    while let Some(event) = ui_rx.recv().await {
        apply_agent_event(agents, handles, event);
    }

    info!("UI consumer stopped");
}

/// Apply an agent event to the agents signal
fn apply_agent_event(
    agents: &GlobalSignal<Vec<AgentSession>>,
    handles: &GlobalSignal<AgentHandles>,
    event: AgentEvent,
) {
    match event {
        AgentEvent::MessageChunk { agent_id, text } => {
            let mut list = agents.write();
            if let Some(agent) = list.iter_mut().find(|a| a.id == agent_id) {
                // Append to existing streaming message or create new one
                if let Some(last_msg) = agent.messages.last_mut() {
                    if last_msg.is_streaming && matches!(last_msg.kind, MessageKind::Text) {
                        last_msg.content.push_str(&text);
                        return;
                    }
                }
                // Create new streaming message
                agent.messages.push(Message {
                    id: uuid::Uuid::new_v4().to_string(),
                    role: Role::Assistant,
                    content: text,
                    kind: MessageKind::Text,
                    timestamp: now_iso(),
                    is_streaming: true,
                });
            }
        }

        AgentEvent::MessageComplete { agent_id } => {
            let mut list = agents.write();
            if let Some(agent) = list.iter_mut().find(|a| a.id == agent_id) {
                if let Some(last_msg) = agent.messages.last_mut() {
                    if last_msg.is_streaming {
                        last_msg.is_streaming = false;
                    }
                }
            }
        }

        AgentEvent::ToolCallStarted {
            agent_id,
            tool_id,
            tool_call,
        } => {
            let mut list = agents.write();
            if let Some(agent) = list.iter_mut().find(|a| a.id == agent_id) {
                // Skip if we already have this tool call
                if agent.tool_calls.contains_key(&tool_id) {
                    return;
                }
                // Skip if message with this ID already exists
                if agent.messages.iter().any(|m| m.id == tool_id) {
                    return;
                }
                // Mark any streaming message as complete
                if let Some(last_msg) = agent.messages.last_mut() {
                    if last_msg.is_streaming {
                        last_msg.is_streaming = false;
                    }
                }
                // Create tool call message
                let input_content = tool_call
                    .raw_input
                    .as_ref()
                    .map(|v| serde_json::to_string_pretty(v).unwrap_or_default())
                    .unwrap_or_default();
                agent.messages.push(Message {
                    id: tool_id.clone(),
                    role: Role::Assistant,
                    content: input_content,
                    kind: MessageKind::ToolCall {
                        name: tool_call.title.clone(),
                        status: ToolCallStatus::Pending,
                        result: None,
                    },
                    timestamp: now_iso(),
                    is_streaming: false,
                });
                // Store tool call for later updates
                agent.tool_calls.insert(tool_id, tool_call);
            }
        }

        AgentEvent::ToolCallUpdated {
            agent_id,
            tool_id,
            fields,
        } => {
            let mut list = agents.write();
            if let Some(agent) = list.iter_mut().find(|a| a.id == agent_id) {
                if let Some(tc) = agent.tool_calls.get_mut(&tool_id) {
                    tc.update(fields);
                }
            }
        }

        AgentEvent::ToolCallCompleted {
            agent_id,
            tool_id,
            result,
        } => {
            let mut list = agents.write();
            if let Some(agent) = list.iter_mut().find(|a| a.id == agent_id) {
                // Find and update existing tool call message
                if let Some(msg) = agent.messages.iter_mut().find(|m| m.id == tool_id) {
                    if let MessageKind::ToolCall {
                        ref mut status,
                        result: ref mut res,
                        ..
                    } = msg.kind
                    {
                        *status = ToolCallStatus::Completed;
                        *res = Some(result);
                    }
                }
            }
        }

        AgentEvent::ToolCallFailed {
            agent_id,
            tool_id,
            error,
        } => {
            let mut list = agents.write();
            if let Some(agent) = list.iter_mut().find(|a| a.id == agent_id) {
                // Find and update existing tool call message
                if let Some(msg) = agent.messages.iter_mut().find(|m| m.id == tool_id) {
                    if let MessageKind::ToolCall {
                        ref mut status,
                        result: ref mut res,
                        ..
                    } = msg.kind
                    {
                        *status = ToolCallStatus::Failed;
                        *res = Some(error);
                    }
                }
            }
        }

        AgentEvent::StatusChange { agent_id, status } => {
            let mut list = agents.write();
            if let Some(agent) = list.iter_mut().find(|a| a.id == agent_id) {
                agent.status = status;
            }
        }

        AgentEvent::PermissionRequest {
            agent_id: _,
            request,
            response_tx,
        } => {
            // TODO: Show permission dialog in UI
            warn!("Auto-approving permission request: {:?}", request.tool_call);
            let response = RequestPermissionResponse {
                outcome: RequestPermissionOutcome::Selected {
                    option_id: request
                        .options
                        .first()
                        .map(|o| o.id.clone())
                        .unwrap_or_else(|| "allow".into()),
                },
                meta: None,
            };
            let _ = response_tx.send(response);
        }

        AgentEvent::Disconnected { agent_id } => {
            let mut list = agents.write();
            if let Some(agent) = list.iter_mut().find(|a| a.id == agent_id) {
                if matches!(agent.status, AgentStatus::Running) {
                    agent.status = AgentStatus::Idle;
                }
            }
            handles.write().remove(&agent_id);
        }

        AgentEvent::Error { agent_id, error } => {
            let mut list = agents.write();
            if let Some(agent) = list.iter_mut().find(|a| a.id == agent_id) {
                agent.status = AgentStatus::Error(error);
            }
        }

        AgentEvent::AvailableCommandsUpdate { agent_id, commands } => {
            let mut list = agents.write();
            if let Some(agent) = list.iter_mut().find(|a| a.id == agent_id) {
                agent.available_commands = commands.into_iter().map(SlashCommand::from).collect();
                info!(
                    agent_id = %agent_id,
                    count = agent.available_commands.len(),
                    "Updated available commands"
                );
            }
        }
    }
}

/// Spawn a new agent on a dedicated thread
async fn create_agent(
    agents: &GlobalSignal<Vec<AgentSession>>,
    handles: &GlobalSignal<AgentHandles>,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    config: AgentConfig,
    initial_message: String,
) -> Result<String, ActorError> {
    let cwd = current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let handle = AgentHandle::spawn(&config.command_line, &cwd, event_tx).await?;

    let agent_id = handle.id.clone();
    let acp_session_id = handle.acp_session_id.clone();

    // Send initial prompt via handle (before storing)
    handle
        .send_prompt(initial_message.clone(), Vec::new())
        .map_err(|e| ActorError::Session(e.to_string()))?;

    // Create UI state
    let session = AgentSession::new(agent_id.clone(), acp_session_id, config, initial_message);
    agents.write().push(session);

    // Store handle for future communication
    handles.write().insert(handle);

    info!(agent_id = %agent_id, "Agent spawned on dedicated thread");

    Ok(agent_id)
}
