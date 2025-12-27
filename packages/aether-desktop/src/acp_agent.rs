use crate::acp_client::{AcpClient, RawAgentEvent};
use crate::state::{AgentStatus, FileReference, SendError};
use agent_client_protocol::{
    Agent, AvailableCommand, ClientSideConnection, ContentBlock, EmbeddedResource,
    EmbeddedResourceResource, InitializeRequest, NewSessionRequest, NewSessionResponse,
    PromptRequest, RequestPermissionRequest, RequestPermissionResponse, SessionId, SessionUpdate,
    TextResourceContents, ToolCall, ToolCallContent, ToolCallStatus, ToolCallUpdateFields, VERSION,
};
use futures::Stream;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::rc::Rc;
use std::thread::JoinHandle;
use tokio::{
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{mpsc, oneshot},
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::{StreamExt, StreamMap};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tracing::{debug, error, info};

/// Runtime handle for an agent process.
///
/// Owns the child process, background thread, and command channel.
/// Not stored in Signal - kept in AgentHandles collection.
pub struct AgentHandle {
    /// Locally-generated UUID for this agent - used as primary identifier in UI
    pub id: String,
    /// ACP session ID - used for protocol communication with child process
    pub acp_session_id: SessionId,
    /// Background thread running the agent (kept for cleanup)
    #[allow(dead_code)]
    thread: JoinHandle<()>,
    /// Command sender for communicating with the agent
    cmd_tx: mpsc::UnboundedSender<AgentCommand>,
}

impl AgentHandle {
    /// Spawn an agent on a dedicated OS thread with its own tokio runtime.
    ///
    /// The agent sends all events through the shared `event_tx` channel.
    /// Awaits until the session is established before returning.
    pub async fn spawn(
        cmd_ref: &str,
        cwd: &Path,
        event_tx: mpsc::UnboundedSender<AgentEvent>,
    ) -> Result<Self, ActorError> {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (init_tx, init_rx) = oneshot::channel();

        // Generate UUID locally before spawning thread
        let agent_id = uuid::Uuid::new_v4().to_string();
        let agent_id_for_thread = agent_id.clone();

        let cmd = cmd_ref.to_string();
        let cwd = cwd.to_path_buf();

        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime for agent");

            // Use LocalSet to support spawn_local for non-Send futures
            let local = tokio::task::LocalSet::new();
            local.block_on(
                &rt,
                run_agent(agent_id_for_thread, cmd, cwd, cmd_rx, event_tx, init_tx),
            );
        });

        // Await initialization (non-blocking)
        let acp_session_id = init_rx
            .await
            .map_err(|_| ActorError::Init("Agent thread terminated during init".into()))??;

        Ok(Self {
            id: agent_id,
            acp_session_id,
            thread,
            cmd_tx,
        })
    }

    /// Send a prompt to the agent.
    pub fn send_prompt(&self, message: String) -> Result<(), SendError> {
        self.send_prompt_with_files(message, Vec::new())
    }

    /// Send a prompt with embedded file references to the agent.
    pub fn send_prompt_with_files(
        &self,
        message: String,
        files: Vec<FileReference>,
    ) -> Result<(), SendError> {
        self.cmd_tx
            .send(AgentCommand::Prompt {
                acp_session_id: self.acp_session_id.clone(),
                message,
                files,
            })
            .map_err(|_| SendError::ChannelClosed)
    }
}

/// UI-ready events emitted by the agent.
///
/// Each variant includes `agent_id` (UUID) for routing to the correct agent in the UI.
/// These are transformed from `RawAgentEvent` by the agent task loop.
#[derive(Debug)]
pub enum AgentEvent {
    /// Append text chunk to the current streaming message
    MessageChunk { agent_id: String, text: String },
    /// Mark current streaming message as complete
    MessageComplete { agent_id: String },
    /// A new tool call started
    ToolCallStarted {
        agent_id: String,
        tool_id: String,
        tool_call: ToolCall,
    },
    /// Tool call fields updated (but not completed/failed)
    ToolCallUpdated {
        agent_id: String,
        tool_id: String,
        fields: ToolCallUpdateFields,
    },
    /// Tool call completed successfully
    ToolCallCompleted {
        agent_id: String,
        tool_id: String,
        result: String,
    },
    /// Tool call failed
    ToolCallFailed {
        agent_id: String,
        tool_id: String,
        error: String,
    },
    /// Agent status changed
    StatusChange {
        agent_id: String,
        status: AgentStatus,
    },
    /// Permission request needs user response
    PermissionRequest {
        #[allow(dead_code)]
        agent_id: String,
        request: RequestPermissionRequest,
        response_tx: oneshot::Sender<RequestPermissionResponse>,
    },
    /// Agent disconnected
    Disconnected { agent_id: String },
    /// Error occurred
    #[allow(dead_code)]
    Error { agent_id: String, error: String },
    /// Available slash commands updated
    AvailableCommandsUpdate {
        agent_id: String,
        commands: Vec<AvailableCommand>,
    },
}

/// Error types for actor operations.
#[derive(Debug)]
pub enum ActorError {
    Spawn(String),
    Init(String),
    Session(String),
}

impl std::fmt::Display for ActorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActorError::Spawn(e) => write!(f, "Failed to spawn agent: {}", e),
            ActorError::Init(e) => write!(f, "Failed to initialize: {}", e),
            ActorError::Session(e) => write!(f, "Failed to create session: {}", e),
        }
    }
}

impl std::error::Error for ActorError {}

/// Commands that can be sent to the ACP actor.
#[derive(Debug)]
pub enum AgentCommand {
    Prompt {
        acp_session_id: SessionId,
        message: String,
        files: Vec<FileReference>,
    },
}

async fn start_session(
    conn: &ClientSideConnection,
    cwd: PathBuf,
) -> Result<NewSessionResponse, ActorError> {
    let init_req = InitializeRequest {
        protocol_version: VERSION,
        client_capabilities: AcpClient::capabilities(),
        meta: None,
    };

    let _ = conn
        .initialize(init_req)
        .await
        .map_err(|e| ActorError::Init(e.to_string()))?;

    let session_req = NewSessionRequest {
        cwd,
        mcp_servers: vec![],
        meta: None,
    };

    let result = conn
        .new_session(session_req)
        .await
        .map_err(|e| ActorError::Session(e.to_string()))?;

    Ok(result)
}

/// Internal event types for the StreamMap
#[derive(Debug)]
enum LoopEvent {
    Command(AgentCommand),
    RawEvent(Box<RawAgentEvent>),
    PromptComplete(Result<agent_client_protocol::PromptResponse, agent_client_protocol::Error>),
}

type EventStream = Pin<Box<dyn Stream<Item = LoopEvent> + Send>>;

/// Main agent loop running on a dedicated thread.
///
/// Handles initialization, sends acp_session_id back via init_tx, then runs the event loop.
async fn run_agent(
    agent_id: String,
    cmd: String,
    cwd: PathBuf,
    cmd_rx: mpsc::UnboundedReceiver<AgentCommand>,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    init_tx: oneshot::Sender<Result<SessionId, ActorError>>,
) {
    // Separate channel for raw events from AcpClient
    let (raw_tx, raw_rx) = mpsc::unbounded_channel::<RawAgentEvent>();

    let (_child, stdin, stdout) = match spawn_child_process(&cmd) {
        Ok(result) => result,
        Err(e) => {
            let _ = init_tx.send(Err(e));
            return;
        }
    };

    let (conn, io_future) = ClientSideConnection::new(
        AcpClient::new(raw_tx),
        stdin.compat_write(),
        stdout.compat(),
        |fut| {
            // Spawn local tasks (LocalBoxFuture is not Send)
            tokio::task::spawn_local(fut);
        },
    );

    // Run IO in the background (also local since io_future is not Send)
    tokio::task::spawn_local(async move {
        if let Err(e) = io_future.await {
            error!("ACP IO error: {}", e);
        }
    });

    let session = match start_session(&conn, cwd).await {
        Ok(session) => session,
        Err(e) => {
            let _ = init_tx.send(Err(e));
            return;
        }
    };
    let acp_session_id = session.session_id.clone();

    // Send acp_session_id back to spawner so it can return
    if init_tx.send(Ok(acp_session_id.clone())).is_err() {
        // Spawner dropped, no point continuing
        return;
    }

    // Wrap conn in Rc for sharing with spawn_local tasks
    let conn = Rc::new(conn);

    // Set up StreamMap for concurrent event handling
    let mut streams: StreamMap<&str, EventStream> = StreamMap::new();

    // Add command stream
    streams.insert(
        "cmd",
        Box::pin(UnboundedReceiverStream::new(cmd_rx).map(LoopEvent::Command)),
    );

    // Add raw event stream
    streams.insert(
        "raw",
        Box::pin(UnboundedReceiverStream::new(raw_rx).map(|e| LoopEvent::RawEvent(Box::new(e)))),
    );

    // Main event loop - polls all streams concurrently
    while let Some((_, event)) = streams.next().await {
        match event {
            LoopEvent::Command(AgentCommand::Prompt {
                acp_session_id: prompt_acp_session_id,
                message,
                files,
            }) => {
                // Spawn prompt as a stream so it doesn't block other events
                let conn = Rc::clone(&conn);
                let (tx, rx) = mpsc::channel(1);

                tokio::task::spawn_local(async move {
                    // Build content blocks: text message + embedded file resources
                    let mut content_blocks: Vec<ContentBlock> = vec![ContentBlock::from(message)];

                    // Read all files in parallel
                    if !files.is_empty() {
                        let file_futures: Vec<_> = files
                            .iter()
                            .map(|file_ref| {
                                let path = file_ref.path.clone();
                                async move { (path.clone(), read_file_for_embedding(&path).await) }
                            })
                            .collect();

                        let results = futures::future::join_all(file_futures).await;

                        for (path, result) in results {
                            match result {
                                Ok(resource_block) => {
                                    content_blocks.push(resource_block);
                                }
                                Err(e) => {
                                    debug!("Failed to read file for embedding: {:?} - {}", path, e);
                                    content_blocks.push(ContentBlock::from(format!(
                                        "[Error reading {}: {}]",
                                        path.display(),
                                        e
                                    )));
                                }
                            }
                        }
                    }

                    let response = conn
                        .prompt(PromptRequest {
                            session_id: prompt_acp_session_id,
                            prompt: content_blocks,
                            meta: None,
                        })
                        .await;
                    let _ = tx.send(response).await;
                });

                streams.insert(
                    "prompt",
                    Box::pin(
                        tokio_stream::wrappers::ReceiverStream::new(rx)
                            .map(LoopEvent::PromptComplete),
                    ),
                );
            }

            LoopEvent::PromptComplete(result) => {
                streams.remove("prompt");
                match result {
                    Ok(response) => {
                        info!(agent_id = %agent_id, "Prompt completed: {:?}", response.stop_reason);
                        let _ = event_tx.send(AgentEvent::MessageComplete {
                            agent_id: agent_id.clone(),
                        });
                        let _ = event_tx.send(AgentEvent::StatusChange {
                            agent_id: agent_id.clone(),
                            status: AgentStatus::Idle,
                        });
                    }
                    Err(e) => {
                        error!(agent_id = %agent_id, "Prompt failed: {}", e);
                        let _ = event_tx.send(AgentEvent::StatusChange {
                            agent_id: agent_id.clone(),
                            status: AgentStatus::Error(e.to_string()),
                        });
                    }
                }
            }

            LoopEvent::RawEvent(raw_event) => {
                let events = transform_raw_event(&agent_id, *raw_event);
                for event in events {
                    let _ = event_tx.send(event);
                }
            }
        }
    }

    let _ = event_tx.send(AgentEvent::Disconnected {
        agent_id: agent_id.clone(),
    });
}

fn spawn_child_process(cmd: &str) -> Result<(Child, ChildStdin, ChildStdout), ActorError> {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() {
        return Err(ActorError::Spawn("Empty command line".to_string()));
    }

    let (command, args) = (parts[0], &parts[1..]);
    debug!("Command: {}, Args: {:?}", command, args);

    let mut child = Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| ActorError::Spawn(e.to_string()))?;

    let stdin = child
        .stdin
        .take()
        .ok_or(ActorError::Spawn("Failed to get stdin".to_string()))?;

    let stdout = child
        .stdout
        .take()
        .ok_or(ActorError::Spawn("Failed to get stdout".to_string()))?;

    Ok((child, stdin, stdout))
}

/// Transform a raw event from AcpClient into UI-ready AgentEvent(s).
///
/// This moves the transformation logic that was previously in EventDispatcher
/// into the agent task loop where we have access to agent_id.
fn transform_raw_event(agent_id: &str, raw_event: RawAgentEvent) -> Vec<AgentEvent> {
    match raw_event {
        RawAgentEvent::SessionNotification(notif) => {
            transform_session_notification(agent_id, notif)
        }
        RawAgentEvent::PermissionRequest {
            request,
            response_tx,
        } => {
            vec![AgentEvent::PermissionRequest {
                agent_id: agent_id.to_string(),
                request,
                response_tx,
            }]
        }
    }
}

fn transform_session_notification(
    agent_id: &str,
    notif: agent_client_protocol::SessionNotification,
) -> Vec<AgentEvent> {
    match notif.update {
        SessionUpdate::AgentMessageChunk { content } => {
            if let ContentBlock::Text(text_content) = content {
                vec![AgentEvent::MessageChunk {
                    agent_id: agent_id.to_string(),
                    text: text_content.text,
                }]
            } else {
                vec![]
            }
        }

        SessionUpdate::UserMessageChunk { content } => {
            if let ContentBlock::Text(text_content) = content {
                debug!("User message chunk: {}", text_content.text);
            }
            vec![]
        }

        SessionUpdate::AgentThoughtChunk { content } => {
            if let ContentBlock::Text(text_content) = content {
                debug!("Agent thought: {}", text_content.text);
            }
            vec![]
        }

        SessionUpdate::ToolCall(tc) => {
            let tool_id = tc.id.0.to_string();
            info!("Tool call started: {} - {}", tool_id, tc.title);

            vec![AgentEvent::ToolCallStarted {
                agent_id: agent_id.to_string(),
                tool_id,
                tool_call: tc,
            }]
        }

        SessionUpdate::ToolCallUpdate(update) => {
            let tool_id = update.id.0.to_string();
            debug!("Tool call update: {} - {:?}", tool_id, update.fields.status);

            match update.fields.status.as_ref() {
                Some(ToolCallStatus::Completed) => {
                    let content = extract_tool_content(&update.fields)
                        .unwrap_or_else(|| "Completed".to_string());
                    vec![AgentEvent::ToolCallCompleted {
                        agent_id: agent_id.to_string(),
                        tool_id,
                        result: content,
                    }]
                }
                Some(ToolCallStatus::Failed) => {
                    let error_msg = extract_tool_content(&update.fields)
                        .unwrap_or_else(|| "Unknown error".to_string());
                    vec![AgentEvent::ToolCallFailed {
                        agent_id: agent_id.to_string(),
                        tool_id,
                        error: error_msg,
                    }]
                }
                _ => vec![AgentEvent::ToolCallUpdated {
                    agent_id: agent_id.to_string(),
                    tool_id,
                    fields: update.fields,
                }],
            }
        }

        SessionUpdate::Plan(plan) => {
            debug!("Received plan: {:?}", plan);
            vec![]
        }

        SessionUpdate::AvailableCommandsUpdate { available_commands } => {
            debug!("Available commands updated: {:?}", available_commands);
            vec![AgentEvent::AvailableCommandsUpdate {
                agent_id: agent_id.to_string(),
                commands: available_commands,
            }]
        }

        SessionUpdate::CurrentModeUpdate { current_mode_id } => {
            debug!("Mode changed to: {}", current_mode_id);
            vec![]
        }
    }
}

/// Extract text content from tool call update fields
fn extract_tool_content(fields: &ToolCallUpdateFields) -> Option<String> {
    fields.content.as_ref().and_then(|contents| {
        contents.iter().find_map(|c| match c {
            ToolCallContent::Content { content: ContentBlock::Text(t) } => Some(t.text.clone()),
            _ => None,
        })
    })
}

/// Read a file and create an embedded resource ContentBlock.
async fn read_file_for_embedding(path: &Path) -> Result<ContentBlock, std::io::Error> {
    let content = tokio::fs::read_to_string(path).await?;
    let uri = format!("file://{}", path.display());

    // Determine MIME type based on extension
    let mime_type = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| match ext.to_lowercase().as_str() {
            "rs" => "text/x-rust",
            "ts" | "tsx" => "text/typescript",
            "js" | "jsx" => "text/javascript",
            "py" => "text/x-python",
            "go" => "text/x-go",
            "java" => "text/x-java",
            "c" | "h" => "text/x-c",
            "cpp" | "hpp" | "cc" => "text/x-c++",
            "json" => "application/json",
            "yaml" | "yml" => "text/yaml",
            "toml" => "text/toml",
            "md" => "text/markdown",
            "html" => "text/html",
            "css" => "text/css",
            "sql" => "text/x-sql",
            "sh" | "bash" => "text/x-shellscript",
            "xml" => "text/xml",
            "proto" => "text/x-protobuf",
            _ => "text/plain",
        })
        .map(String::from);

    Ok(ContentBlock::Resource(EmbeddedResource {
        annotations: None,
        resource: EmbeddedResourceResource::TextResourceContents(TextResourceContents {
            uri,
            mime_type,
            text: content,
            meta: None,
        }),
        meta: None,
    }))
}
