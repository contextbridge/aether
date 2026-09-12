use acp_utils::server::AcpServerError;
use aether_sessions::{SessionEvent, UserEvent};
use agent_client_protocol::schema::v2::{SessionId, SessionUpdate, UpdateSessionNotification};
use agent_client_protocol::{Client, ConnectionTo};

use super::content::map_user_message;
use super::events::{NotificationMode, map_agent_event_to_notification};

/// Replay session events to the client as ACP notifications.
///
/// Replays message upserts. Partial chunks are omitted.
pub fn replay_to_client(events: &[SessionEvent], connection: &ConnectionTo<Client>, session_id: &SessionId) {
    for event in events {
        for notif in map_session_event_to_notifications(event, session_id) {
            if let Err(e) =
                connection.send_notification(notif).map_err(|e| AcpServerError::protocol("session/update", e))
            {
                tracing::error!("Failed to send replay notification: {e:?}");
            }
        }
    }
}

pub fn map_session_event_to_notifications(
    event: &SessionEvent,
    session_id: &SessionId,
) -> Vec<UpdateSessionNotification> {
    match event {
        SessionEvent::User(UserEvent::Message { message_id, content }) => vec![UpdateSessionNotification::new(
            session_id.clone(),
            SessionUpdate::UserMessage(map_user_message(message_id.as_str().into(), content)),
        )],
        SessionEvent::Agent(message) => {
            map_agent_event_to_notification(session_id.clone(), message, NotificationMode::Replay).into_iter().collect()
        }
        SessionEvent::User(_) | SessionEvent::Control(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v2 as acp;

    #[test]
    fn replay_emits_one_user_upsert_with_media_in_order_and_stable_identity() {
        let session_id = acp::SessionId::new("test-session");
        let event = SessionEvent::User(UserEvent::Message {
            message_id: "user".into(),
            content: vec![
                llm::ContentBlock::text("hello"),
                llm::ContentBlock::Image { data: "aW1n".to_string(), mime_type: "image/png".to_string() },
                llm::ContentBlock::Audio { data: "YXVkaW8=".to_string(), mime_type: "audio/wav".to_string() },
            ],
        });
        let first = map_session_event_to_notifications(&event, &session_id);
        let second = map_session_event_to_notifications(&event, &session_id);
        assert_eq!(first.len(), 1);
        let SessionUpdate::UserMessage(message) = &first[0].update else { panic!("expected user upsert") };
        let SessionUpdate::UserMessage(replayed) = &second[0].update else { panic!("expected user upsert") };
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
        assert!(map_session_event_to_notifications(&event, &SessionId::new("test")).is_empty());
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
            replay_to_client(&events, &cx, &session_id);
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
