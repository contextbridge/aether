use crate::acp::session::actor::SessionIo;
use aether_sessions::{SessionEvent, UserEvent};
use agent_client_protocol::schema::v2::SessionUpdate;

use super::content::map_user_message;
use super::events::{NotificationMode, project_agent_event};

/// Replay events, omitting partial chunks.
pub(crate) fn replay_to_client(events: &[SessionEvent], io: &SessionIo) {
    for event in events {
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
