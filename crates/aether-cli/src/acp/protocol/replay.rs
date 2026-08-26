use acp_utils::server::AcpServerError;
use aether_sessions::{SessionEvent, UserEvent};
use agent_client_protocol::schema::v1::{ContentChunk, SessionId, SessionNotification, SessionUpdate};
use agent_client_protocol::{Client, ConnectionTo};

use super::content::map_user_content_block;
use super::events::{NotificationMode, map_agent_event_to_notification};

/// Replay session events to the client as ACP notifications.
pub async fn replay_to_client(events: &[SessionEvent], connection: &ConnectionTo<Client>, session_id: &SessionId) {
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

pub fn map_session_event_to_notifications(event: &SessionEvent, session_id: &SessionId) -> Vec<SessionNotification> {
    match event {
        SessionEvent::User(UserEvent::Message { content }) => content
            .iter()
            .map(|block| {
                SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::UserMessageChunk(ContentChunk::new(map_user_content_block(block))),
                )
            })
            .collect(),
        SessionEvent::Agent(message) => {
            map_agent_event_to_notification(session_id.clone(), message, NotificationMode::Replay).into_iter().collect()
        }
        SessionEvent::User(_) | SessionEvent::Control(_) => Vec::new(),
    }
}

#[cfg(test)]
pub fn map_session_events_to_notifications(
    events: &[SessionEvent],
    session_id: &SessionId,
) -> Vec<SessionNotification> {
    events.iter().flat_map(|event| map_session_event_to_notifications(event, session_id)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1 as acp;

    #[test]
    fn replay_emits_user_media_chunks_in_order() {
        let session_id = acp::SessionId::new("test-session");
        let events = vec![SessionEvent::User(UserEvent::Message {
            content: vec![
                llm::ContentBlock::text("hello"),
                llm::ContentBlock::Image { data: "aW1n".to_string(), mime_type: "image/png".to_string() },
                llm::ContentBlock::Audio { data: "YXVkaW8=".to_string(), mime_type: "audio/wav".to_string() },
            ],
        })];

        let notifications = map_session_events_to_notifications(&events, &session_id);
        let updates: Vec<_> = notifications.into_iter().map(|n| n.update).collect();
        assert!(matches!(
            &updates[0],
            acp::SessionUpdate::UserMessageChunk(chunk)
                if matches!(&chunk.content, acp::ContentBlock::Text(text) if text.text == "hello")
        ));
        assert!(matches!(
            &updates[1],
            acp::SessionUpdate::UserMessageChunk(chunk)
                if matches!(&chunk.content, acp::ContentBlock::Image(_))
        ));
        assert!(matches!(
            &updates[2],
            acp::SessionUpdate::UserMessageChunk(chunk)
                if matches!(&chunk.content, acp::ContentBlock::Audio(_))
        ));
    }

    #[test]
    fn replay_ignores_control_events() {
        use aether_sessions::SessionControlEvent;

        let session_id = acp::SessionId::new("test-session");
        let events = vec![SessionEvent::Control(SessionControlEvent::AgentSwitched {
            from: Some("Planner".to_string()),
            to: Some("Coder".to_string()),
        })];

        let notifications = map_session_events_to_notifications(&events, &session_id);
        assert!(notifications.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn replay_to_client_forwards_each_event_as_session_notification() {
        use acp_utils::testing::test_connection;
        use llm::ContentBlock as LlmContentBlock;
        use tokio::task::LocalSet;

        LocalSet::new()
            .run_until(async {
                let (cx, mut peer) = test_connection().await;
                let session_id = acp::SessionId::new("test-session");
                let events = vec![SessionEvent::User(UserEvent::Message {
                    content: vec![LlmContentBlock::text("hello"), LlmContentBlock::text("world")],
                })];

                replay_to_client(&events, &cx, &session_id).await;

                for _ in 0..2 {
                    let notif = peer.next_session_notification().await;
                    assert_eq!(notif.session_id, session_id);
                    assert!(matches!(notif.update, SessionUpdate::UserMessageChunk(_)));
                }
            })
            .await;
    }
}
