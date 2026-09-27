use crate::ClientError;
use crate::js::to_js;
use crate::types::ConversationSnapshot;
use acp_utils::conversation::{Conversation, ConversationItemId, Revision};
use agent_client_protocol::schema::v2::SessionId;
use js_sys::Array;
use wasm_bindgen::JsValue;

/// A session's conversation, with the JavaScript objects of its items kept between snapshots.
pub(crate) struct TrackedConversation {
    pub(crate) session_id: SessionId,
    pub(crate) conversation: Conversation,
    emitted: Option<Revision>,
    items: Vec<ConvertedItem>,
}

impl TrackedConversation {
    pub(crate) fn new(session_id: SessionId) -> Self {
        Self { session_id, conversation: Conversation::new(), emitted: None, items: Vec::new() }
    }

    pub(crate) fn changed(&mut self) -> Option<Result<ConversationSnapshot<'_>, ClientError>> {
        let revision = self.conversation.revision();
        if self.emitted == Some(revision) {
            return None;
        }
        self.emitted = Some(revision);
        Some(self.snapshot())
    }

    fn snapshot(&mut self) -> Result<ConversationSnapshot<'_>, ClientError> {
        let items = self
            .conversation
            .items()
            .iter()
            .enumerate()
            .map(|(index, item)| match self.items.get(index) {
                Some(converted) if converted.id == item.id() && converted.revision == item.revision() => {
                    Ok(ConvertedItem { value: converted.value.clone(), ..*converted })
                }
                _ => Ok(ConvertedItem { id: item.id(), revision: item.revision(), value: to_js(item)? }),
            })
            .collect::<Result<Vec<_>, ClientError>>()?;

        let array = items.iter().map(|item| item.value.clone()).collect::<Array>();
        self.items = items;
        let conversation = &self.conversation;

        Ok(ConversationSnapshot {
            session_id: &self.session_id,
            items: array,
            turn: conversation.turn(),
            activity: conversation.activity(),
            plan: conversation.plan(),
            context_usage: conversation.context_usage(),
            compacting: conversation.is_compacting(),
        })
    }
}

struct ConvertedItem {
    id: ConversationItemId,
    revision: Revision,
    value: JsValue,
}
