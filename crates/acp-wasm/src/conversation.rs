use crate::{ClientError, to_js};
use acp_utils::client::{AcpClientError, AcpEvent};
use acp_utils::conversation::{
    Activity, Conversation, ConversationId, ConversationItem, ConversationItemId, Revision, TurnPhase,
};
use agent_client_protocol::schema::v2::{self as acp, SessionId};
use js_sys::{Array, Function, Reflect};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::HashMap;
use wasm_bindgen::JsValue;

/// What an `AetherClient` shares with its event pump: the JS event callback and
/// the conversation of every session it has seen.
pub(crate) struct ClientState {
    on_event: Function,
    sessions: RefCell<HashMap<SessionId, TrackedConversation>>,
}

impl ClientState {
    pub(crate) fn new(on_event: Function) -> Self {
        Self { on_event, sessions: RefCell::new(HashMap::new()) }
    }

    pub(crate) fn emit(&self, event: &JsValue) {
        let _ = self.on_event.call1(&JsValue::NULL, event);
    }

    /// Reduce an agent event into the conversations it concerns and return their sessions.
    pub(crate) fn reduce(&self, event: &AcpEvent) -> Vec<SessionId> {
        let mut sessions = self.sessions.borrow_mut();
        match event {
            AcpEvent::SessionUpdate(notification) => {
                let tracked = sessions.entry(notification.session_id.clone()).or_default();
                tracked.conversation.apply_update(&notification.update);
                vec![notification.session_id.clone()]
            }
            // Aether's own notifications carry no session id, so every conversation sees them.
            AcpEvent::SubAgentProgress(progress) => {
                reduce_all(&mut sessions, |conversation| conversation.apply_sub_agent_progress(progress))
            }
            AcpEvent::ContextCleared(_) => reduce_all(&mut sessions, Conversation::clear),
            AcpEvent::ConnectionClosed => reduce_all(&mut sessions, Conversation::connection_closed),
            AcpEvent::AuthMethodsUpdated(_)
            | AcpEvent::McpNotification(_)
            | AcpEvent::GitDiffEvent(_)
            | AcpEvent::ElicitationRequest { .. } => Vec::new(),
        }
    }

    /// Track a session, keeping anything already reduced for it.
    pub(crate) fn track(&self, session_id: SessionId) {
        self.sessions.borrow_mut().entry(session_id).or_default();
    }

    /// Start the session's conversation over, as its resumed history is about to be replayed.
    pub(crate) fn restart(&self, session_id: &SessionId) {
        self.sessions.borrow_mut().insert(session_id.clone(), TrackedConversation::default());
        self.emit_changed(session_id);
    }

    /// Echo a prompt and start its turn, unless the session is already in one.
    pub(crate) fn start_prompt(
        &self,
        session_id: &SessionId,
        content: Vec<acp::ContentBlock>,
    ) -> Result<(), ClientError> {
        {
            let mut sessions = self.sessions.borrow_mut();
            let conversation = &mut sessions.entry(session_id.clone()).or_default().conversation;
            if !conversation.turn().is_idle() {
                return Err(ClientError::TurnInProgress);
            }
            conversation.append_pending_user_content(content);
            conversation.start_prompt();
        }
        self.emit_changed(session_id);
        Ok(())
    }

    pub(crate) fn settle_prompt(&self, session_id: &SessionId, result: Result<&acp::PromptResponse, &AcpClientError>) {
        {
            let mut sessions = self.sessions.borrow_mut();
            let conversation = &mut sessions.entry(session_id.clone()).or_default().conversation;
            match result {
                Ok(_) => conversation.accept_prompt(),
                Err(error) => conversation.reject_prompt(&error.to_string()),
            }
        }
        self.emit_changed(session_id);
    }

    /// The session's conversation as a JS object, if the session is tracked.
    pub(crate) fn snapshot(&self, session_id: &SessionId) -> Result<Option<JsValue>, ClientError> {
        self.sessions.borrow_mut().get_mut(session_id).map(TrackedConversation::snapshot).transpose()
    }

    /// Deliver the session's current conversation as a `conversation_changed` event.
    pub(crate) fn emit_changed(&self, session_id: &SessionId) {
        let Ok(Some(conversation)) = self.snapshot(session_id) else {
            return;
        };
        let Ok(event) = to_js(&ConversationChanged { r#type: "conversation_changed", session_id }) else {
            return;
        };
        let _ = Reflect::set(&event, &"conversation".into(), &conversation);
        self.emit(&event);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationChanged<'a> {
    r#type: &'static str,
    session_id: &'a SessionId,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationFields<'a> {
    turn: TurnPhase,
    activity: &'a Activity,
    plan: Option<&'a acp::PlanItems>,
    context_usage: Option<&'a acp::UsageUpdate>,
    compacting: bool,
}

/// A conversation plus the JS objects of its items, so an unchanged item is the
/// same object in every snapshot and only changed items are converted again.
#[derive(Default)]
struct TrackedConversation {
    conversation: Conversation,
    converted: Option<ConversationId>,
    items: Vec<ConvertedItem>,
}

struct ConvertedItem {
    id: ConversationItemId,
    revision: Revision,
    value: JsValue,
}

impl TrackedConversation {
    fn snapshot(&mut self) -> Result<JsValue, ClientError> {
        if self.converted != Some(self.conversation.id()) {
            self.items.clear();
            self.converted = Some(self.conversation.id());
        }
        let items = self
            .conversation
            .items()
            .iter()
            .enumerate()
            .map(|(index, item)| match self.items.get(index) {
                Some(converted) if converted.id == item.id() && converted.revision == item.revision() => {
                    Ok(ConvertedItem { value: converted.value.clone(), ..*converted })
                }
                _ => convert(item),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let snapshot = to_js(&ConversationFields {
            turn: self.conversation.turn(),
            activity: self.conversation.activity(),
            plan: self.conversation.plan(),
            context_usage: self.conversation.context_usage(),
            compacting: self.conversation.is_compacting(),
        })?;
        let array = items.iter().map(|item| item.value.clone()).collect::<Array>();
        let _ = Reflect::set(&snapshot, &"items".into(), &array);
        self.items = items;
        Ok(snapshot)
    }
}

fn reduce_all(
    sessions: &mut HashMap<SessionId, TrackedConversation>,
    apply: impl Fn(&mut Conversation),
) -> Vec<SessionId> {
    sessions
        .iter_mut()
        .map(|(session_id, tracked)| {
            apply(&mut tracked.conversation);
            session_id.clone()
        })
        .collect()
}

fn convert(item: &ConversationItem) -> Result<ConvertedItem, ClientError> {
    Ok(ConvertedItem { id: item.id(), revision: item.revision(), value: to_js(item)? })
}
