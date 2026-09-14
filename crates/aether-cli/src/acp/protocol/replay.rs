use crate::acp::session::actor::SessionIo;
use aether_core::events::{AgentEvent, CompactionId, ContextEvent};
use aether_sessions::{SessionEvent, UserEvent};
use agent_client_protocol::schema::v2::{CompactionStatus, CompactionUpdate, SessionUpdate};
use std::collections::HashMap;

use super::content::map_user_message;
use super::events::{NotificationMode, map_agent_event_to_notification, project_agent_event};

pub(crate) fn replay_to_client(events: &[SessionEvent], io: &SessionIo) {
    let mut compactions: HashMap<&CompactionId, (usize, Option<CompactionUpdate>)> = HashMap::new();
    for (position, event) in events.iter().enumerate() {
        if let Some(id) = compaction_id(event) {
            let entry = compactions.entry(id).or_insert((position, None));
            if let SessionEvent::Agent(message) = event
                && let Some(SessionUpdate::CompactionUpdate(update)) =
                    map_agent_event_to_notification(message, NotificationMode::Replay)
                && matches!(
                    update.status,
                    CompactionStatus::Completed | CompactionStatus::Failed | CompactionStatus::Cancelled
                )
            {
                entry.1 = Some(update);
            }
        }
    }
    for (position, event) in events.iter().enumerate() {
        if let Some(id) = compaction_id(event) {
            if let Some((first, update)) = compactions.get_mut(id)
                && *first == position
                && let Some(update) = update.take()
            {
                io.send_update(SessionUpdate::CompactionUpdate(update));
            }
            continue;
        }
        match event {
            SessionEvent::User(UserEvent::Message { message_id, content, display_content }) => {
                io.send_update(SessionUpdate::UserMessage(map_user_message(
                    message_id.as_str().into(),
                    display_content.as_deref().unwrap_or(content),
                )));
            }
            SessionEvent::Agent(message) => project_agent_event(message, NotificationMode::Replay, io),
            SessionEvent::User(_) | SessionEvent::Control(_) => {}
        }
    }
}

fn compaction_id(event: &SessionEvent) -> Option<&CompactionId> {
    match event {
        SessionEvent::Agent(AgentEvent::Context(
            ContextEvent::CompactionStarted { compaction_id, .. }
            | ContextEvent::CompactionResult { compaction_id, .. }
            | ContextEvent::CompactionEnded { compaction_id, .. },
        )) => Some(compaction_id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::events::{AgentEvent, MessageEvent};
    use agent_client_protocol::schema::v2::{self as acp, SessionId};

    #[tokio::test]
    async fn replay_preserves_message_identity_and_media_without_chunks_or_controls() {
        tokio::task::LocalSet::new().run_until(async {
            let (cx, mut peer) = acp_utils::testing::test_connection().await;
            let session_id = SessionId::new("test");
            let io = SessionIo::new(cx, session_id.clone());
            let events = vec![
                SessionEvent::User(UserEvent::Message {
                    message_id: "user".into(),
                    display_content: None,
                    content: vec![
                        llm::ContentBlock::text("hello"),
                        llm::ContentBlock::Image { data: "aW1n".into(), mime_type: "image/png".into() },
                        llm::ContentBlock::Audio { data: "YXVkaW8=".into(), mime_type: "audio/wav".into() },
                    ],
                }),
                SessionEvent::Control(aether_sessions::SessionControlEvent::AgentSwitched {
                    from: Some("Planner".into()), to: Some("Coder".into()),
                }),
                SessionEvent::Agent(AgentEvent::Message(MessageEvent::Text {
                    message_id: "original".into(), chunk: "reply".into(), is_complete: false,
                })),
                SessionEvent::Agent(AgentEvent::Message(MessageEvent::Text {
                    message_id: "original".into(), chunk: "reply".into(), is_complete: true,
                })),
            ];
            for _ in 0..2 {
                replay_to_client(&events, &io);
                let first = peer.next_session_notification().await;
                assert_eq!(first.session_id, session_id);
                let SessionUpdate::UserMessage(message) = first.update else { panic!("expected user upsert") };
                assert_eq!(message.message_id.0.as_ref(), "user");
                let content = message.content.value().unwrap();
                assert!(matches!(&content[0], acp::ContentBlock::Text(text) if text.text == "hello"));
                assert!(matches!(&content[1], acp::ContentBlock::Image(_)));
                assert!(matches!(&content[2], acp::ContentBlock::Audio(_)));
                let second = peer.next_session_notification().await;
                let SessionUpdate::AgentMessage(message) = second.update else { panic!("expected whole message") };
                assert_eq!(message.message_id.0.as_ref(), "original");
                assert!(matches!(&message.content.value().unwrap()[0], acp::ContentBlock::Text(text) if text.text == "reply"));
            }
        }).await;
    }
}
