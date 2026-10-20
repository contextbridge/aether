pub use utils::ReasoningEffort;

/// Source metadata for the model's explicit disabling capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningDisabledSupport {
    Unsupported,
    Effort,
    Toggle,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ReasoningControl {
    #[default]
    Effort,
    Budget,
    BudgetAndEffort,
}
