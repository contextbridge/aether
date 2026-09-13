use crate::acp::session::actor::SessionIo;
use aether_sessions::{SessionEvent, UserEvent};
use agent_client_protocol::schema::v2::SessionUpdate;

use super::content::map_user_message;
use super::events::{NotificationMode, map_agent_event_to_notification};

/// Replay session events to the client as ACP notifications.
///
/// Replays message upserts. Partial chunks are omitted.
pub(crate) fn replay_to_client(events: &[SessionEvent], io: &SessionIo) {
    for event in events {
        if let Some(update) = map_session_event_to_notifications(event) {
            io.send_update(update);
        }
    }
}

pub fn map_session_event_to_notifications(event: &SessionEvent) -> Option<SessionUpdate> {
    match event {
        SessionEvent::User(UserEvent::Message { message_id, content }) => {
            Some(SessionUpdate::UserMessage(map_user_message(message_id.as_str().into(), content)))
        }
        SessionEvent::Agent(message) => map_agent_event_to_notification(message, NotificationMode::Replay),
        SessionEvent::User(_) | SessionEvent::Control(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v2::{self as acp, SessionId};

    #[test]
    fn replay_emits_one_user_upsert_with_media_in_order_and_stable_identity() {
        let event = SessionEvent::User(UserEvent::Message {
            message_id: "user".into(),
            content: vec![
                llm::ContentBlock::text("hello"),
                llm::ContentBlock::Image { data: "aW1n".to_string(), mime_type: "image/png".to_string() },
                llm::ContentBlock::Audio { data: "YXVkaW8=".to_string(), mime_type: "audio/wav".to_string() },
            ],
        });
        let first = map_session_event_to_notifications(&event).unwrap();
        let second = map_session_event_to_notifications(&event).unwrap();
        let SessionUpdate::UserMessage(message) = &first else { panic!("expected user upsert") };
        let SessionUpdate::UserMessage(replayed) = &second else { panic!("expected user upsert") };
        assert_eq!(message.message_id, replayed.message_id);
        let content = message.content.value().expect("whole message content");
        assert!(matches!(&content[0], acp::ContentBlock::Text(text) if text.text == "hello"));
        assert!(matches!(&content[1], acp::ContentBlock::Image(_)));
        assert!(matches!(&content[2], acp::ContentBlock::Audio(_)));
    }

    #[test]
    fn replay_ignores_control_events() {
        let event = SessionEvent::Control(aether_sessions::SessionControlEvent::AgentSwitched {
            from: Some("Planner".to_string()),
            to: Some("Coder".to_string()),
        });
        assert!(map_session_event_to_notifications(&event).is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn replay_to_client_forwards_whole_messages_without_chunks() {
        use aether_core::events::{AgentEvent, MessageEvent};
        tokio::task::LocalSet::new().run_until(async {
            let (cx, mut peer) = acp_utils::testing::test_connection().await;
            let session_id = SessionId::new("test");
            let events = vec![
                SessionEvent::User(UserEvent::Message { message_id: "user".into(), content: vec![llm::ContentBlock::text("hello"), llm::ContentBlock::text("world")] }),
                SessionEvent::Agent(AgentEvent::Message(MessageEvent::Text { message_id: "original".into(), chunk: "reply".into(), is_complete: false })),
                SessionEvent::Agent(AgentEvent::Message(MessageEvent::Text { message_id: "original".into(), chunk: "reply".into(), is_complete: true })),
            ];
            replay_to_client(&events, &SessionIo::new(cx, session_id.clone()));
            let first = peer.next_session_notification().await;
            assert_eq!(first.session_id, session_id);
            assert!(matches!(first.update, SessionUpdate::UserMessage(_)));
            let second = peer.next_session_notification().await;
            let SessionUpdate::AgentMessage(message) = second.update else { panic!("expected complete agent message") };
            assert_eq!(message.message_id.0.as_ref(), "original");
            assert!(matches!(&message.content.value().unwrap()[0], acp::ContentBlock::Text(text) if text.text == "reply"));
        }).await;
    }
}
