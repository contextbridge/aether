pub(crate) use acp_utils::content::{map_acp_to_content_blocks, map_user_content_block};
use agent_client_protocol::schema::v2 as acp;
use llm::ContentBlock as LlmContentBlock;

pub(crate) fn map_user_message(message_id: acp::MessageId, blocks: &[LlmContentBlock]) -> acp::UserMessage {
    acp::UserMessage::new(message_id).content(blocks.iter().map(map_user_content_block).collect::<Vec<_>>())
}
