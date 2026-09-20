pub use utils::ReasoningEffort;

/// Source metadata for the model's explicit disabling capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningDisabledSupport {
    Unsupported,
    Effort,
    Toggle,
}
