/// Executable per-model transport strategy.
///
/// A model without an override uses its provider's default transport. Each
/// variant here is a complete routing decision implemented by a runtime provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelTransport {
    /// `OpenAI` Responses API at a dedicated endpoint template.
    OpenAiResponses { base_url_template: &'static str },
}
