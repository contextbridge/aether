use crate::AppEvent;
use crate::agent_session::AgentSession;
use crate::files::build_file_content_blocks;
use agent_client_protocol::schema::v1::{
    ContentBlock, ListSessionsRequest, PromptRequest, SessionConfigOption, SessionConfigOptionValue, SessionId,
    SetSessionConfigOptionRequest, TextContent,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::ipc::Channel;

#[derive(Default)]
pub(crate) struct AppState {
    sessions: Mutex<HashMap<String, AgentSession>>,
    git_mutations: tokio::sync::Mutex<()>,
}

pub(crate) struct AgentSessionDescriptor {
    pub(crate) connection_id: String,
    pub(crate) session_id: SessionId,
    pub(crate) agent_name: String,
    pub(crate) config_options: Vec<SessionConfigOption>,
}

impl AppState {
    pub(crate) async fn start_session(
        self: &Arc<Self>,
        program: String,
        args: Vec<String>,
        cwd: String,
        events: Channel<AppEvent>,
    ) -> Result<AgentSessionDescriptor, String> {
        let cwd = PathBuf::from(cwd);
        if !cwd.is_dir() {
            return Err(format!("working directory does not exist: {}", cwd.display()));
        }

        let (session, mut ended_rx) = AgentSession::spawn(program, args, cwd, events).await?;
        let session_key = session.session_id.0.to_string();
        let (descriptor, session_id, connection_id) = {
            let mut sessions = self.sessions.lock().map_err(|_| "app state lock poisoned".to_string())?;
            if sessions.contains_key(&session_key) {
                return Err("session is already active".to_string());
            }

            let descriptor = AgentSessionDescriptor {
                connection_id: session.connection_id.clone(),
                session_id: session.session_id.clone(),
                agent_name: session.agent_name.clone(),
                config_options: session.config_options.clone(),
            };

            let session_id = session.session_id.0.to_string();
            let connection_id = session.connection_id.clone();
            sessions.insert(session_key, session);
            (descriptor, session_id, connection_id)
        };

        let state = Arc::clone(self);
        tokio::spawn(async move {
            if ended_rx.changed().await.is_ok() {
                state.remove_if_current(&session_id, &connection_id);
            }
        });

        Ok(descriptor)
    }

    pub(crate) async fn load_session(
        self: &Arc<Self>,
        program: String,
        args: Vec<String>,
        session_id: String,
        cwd: String,
        events: Channel<AppEvent>,
    ) -> Result<AgentSessionDescriptor, String> {
        let cwd = PathBuf::from(cwd);
        if !cwd.is_dir() {
            return Err(format!("working directory does not exist: {}", cwd.display()));
        }
        if self.sessions.lock().map_err(|_| "app state lock poisoned".to_string())?.contains_key(&session_id) {
            return Err("session is already active".to_string());
        }

        let (session, mut ended_rx) =
            AgentSession::spawn_loaded(program, args, SessionId::new(session_id), cwd, events).await?;
        let session_key = session.session_id.0.to_string();
        let descriptor = AgentSessionDescriptor {
            connection_id: session.connection_id.clone(),
            session_id: session.session_id.clone(),
            agent_name: session.agent_name.clone(),
            config_options: session.config_options.clone(),
        };
        let connection_id = session.connection_id.clone();
        self.sessions.lock().map_err(|_| "app state lock poisoned".to_string())?.insert(session_key.clone(), session);

        let state = Arc::clone(self);
        tokio::spawn(async move {
            if ended_rx.changed().await.is_ok() {
                state.remove_if_current(&session_key, &connection_id);
            }
        });
        Ok(descriptor)
    }

    pub(crate) fn list_sessions(&self, session_id: &str) -> Result<(), String> {
        let client_handle = {
            let sessions = self.sessions.lock().map_err(|_| "app state lock poisoned".to_string())?;
            sessions.get(session_id).ok_or_else(|| "session is no longer active".to_string())?.client_handle.clone()
        };
        tokio::spawn(async move {
            if let Err(error) = client_handle.list_sessions_with_event(ListSessionsRequest::new()).await {
                tracing::warn!(%error, "failed to list ACP sessions");
            }
        });
        Ok(())
    }

    pub(crate) fn set_session_config_option(
        &self,
        session_id: &str,
        config_id: &str,
        value: &str,
    ) -> Result<(), String> {
        let (client_handle, session_id) = {
            let sessions = self.sessions.lock().map_err(|_| "app state lock poisoned".to_string())?;
            let session = sessions.get(session_id).ok_or_else(|| "session is no longer active".to_string())?;
            (session.client_handle.clone(), session.session_id.clone())
        };
        let request = SetSessionConfigOptionRequest::new(
            session_id,
            config_id.to_string(),
            SessionConfigOptionValue::value_id(value.to_string()),
        );
        tokio::spawn(async move {
            if let Err(error) = client_handle.set_config_option(request).await {
                tracing::warn!(%error, "failed to set ACP session config option");
            }
        });
        Ok(())
    }

    pub(crate) fn send_prompt(
        &self,
        session_id: &str,
        text: &str,
        file_paths: Option<&[String]>,
    ) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("prompt cannot be empty".to_string());
        }

        let (client_handle, session_id, cwd) = {
            let sessions = self.sessions.lock().map_err(|_| "app state lock poisoned".to_string())?;
            let session = sessions.get(session_id).ok_or_else(|| "session is no longer active".to_string())?;
            (session.client_handle.clone(), session.session_id.clone(), session.cwd.clone())
        };

        let mut prompt = vec![ContentBlock::Text(TextContent::new(text))];
        if let Some(content) = file_paths.map(|paths| build_file_content_blocks(&cwd, paths)).transpose()? {
            prompt.extend(content);
        }
        let request = PromptRequest::new(session_id, prompt);
        tokio::spawn(async move {
            if let Err(error) = client_handle.prompt(request).await {
                tracing::warn!(%error, "ACP prompt failed");
            }
        });
        Ok(())
    }

    pub(crate) fn cancel_prompt(&self, session_id: &str) -> Result<(), String> {
        let (client_handle, session_id) = {
            let sessions = self.sessions.lock().map_err(|_| "app state lock poisoned".to_string())?;
            let session = sessions.get(session_id).ok_or_else(|| "session is no longer active".to_string())?;
            (session.client_handle.clone(), session.session_id.clone())
        };
        tokio::spawn(async move {
            if let Err(error) =
                client_handle.cancel(agent_client_protocol::schema::v1::CancelNotification::new(session_id)).await
            {
                tracing::warn!(%error, "failed to cancel ACP prompt");
            }
        });
        Ok(())
    }

    pub(crate) async fn lock_git_mutations(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.git_mutations.lock().await
    }

    pub(crate) fn working_directory(&self, session_id: &str) -> Result<PathBuf, String> {
        let sessions = self.sessions.lock().map_err(|_| "app state lock poisoned".to_string())?;
        sessions
            .get(session_id)
            .map(|session| session.cwd.clone())
            .ok_or_else(|| "session is no longer active".to_string())
    }

    pub(crate) fn close_session(&self, session_id: &str) -> Result<(), String> {
        let _session = {
            let mut sessions = self.sessions.lock().map_err(|_| "app state lock poisoned".to_string())?;
            sessions.remove(session_id).ok_or_else(|| "session is no longer active".to_string())?
        };

        Ok(())
    }

    pub(crate) fn close_all_sessions(&self) {
        let _sessions = {
            let mut sessions = self.sessions.lock().expect("app state lock poisoned");
            sessions.drain().map(|(_, session)| session).collect::<Vec<_>>()
        };
    }

    pub(crate) fn remove_if_current(&self, session_id: &str, connection_id: &str) {
        let mut sessions = self.sessions.lock().expect("app state lock poisoned");
        if sessions.get(session_id).is_some_and(|session| session.connection_id == connection_id) {
            sessions.remove(session_id);
        }
    }
}
