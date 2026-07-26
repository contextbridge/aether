use base16ct::lower::encode_string;
use llm::{ChatMessage, Context, StreamingModelProvider};
use sha2::{Digest, Sha256};

/// Derive a provider routing key from the cacheable request prefix.
///
/// Requests sharing a prefix hash to the same key so the provider routes them to
/// the same machine, where the prefix cache is already warm. Every input is
/// length-prefixed so adjacent fields cannot be confused for one another.
pub(super) fn derive_prompt_cache_key(llm: &dyn StreamingModelProvider, context: &Context) -> String {
    let mut hasher = Sha256::new();
    let model = llm.model().map_or_else(|| llm.display_name(), |model| model.to_string());
    update_hash(&mut hasher, model.as_bytes());

    for message in context.messages() {
        if let ChatMessage::System { content, .. } = message {
            update_hash(&mut hasher, content.as_bytes());
        }
    }

    for tool in context.tools() {
        update_hash(&mut hasher, tool.name.as_bytes());
        update_hash(&mut hasher, tool.description.as_bytes());
        update_hash(&mut hasher, tool.parameters.to_string().as_bytes());
    }

    encode_string(hasher.finalize().as_ref())
}

fn update_hash(hasher: &mut Sha256, value: &[u8]) {
    hasher.update(value.len().to_be_bytes());
    hasher.update(value);
}
