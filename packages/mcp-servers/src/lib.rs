#![doc = include_str!("../README.md")]

#[cfg(any(feature = "all", feature = "subagents"))]
pub mod setup;
#[cfg(any(feature = "all", feature = "subagents"))]
pub use setup::McpBuilderExt;

#[cfg(feature = "coding")]
pub mod coding;

#[cfg(feature = "coding")]
pub mod lsp;

#[cfg(feature = "skills")]
pub mod skills;

#[cfg(feature = "tasks")]
pub mod tasks;

#[cfg(feature = "subagents")]
pub mod subagents;

#[cfg(feature = "survey")]
pub mod survey;

#[cfg(feature = "plan")]
pub mod plan;

// Re-export primary types for convenience
#[cfg(feature = "coding")]
pub use coding::{CodingMcp, CodingMcpArgs, CodingTools, DefaultCodingTools, PermissionMode};

#[cfg(feature = "coding")]
pub use lsp::LspRegistry;

#[cfg(feature = "skills")]
pub use skills::{SkillsMcp, SkillsMcpArgs};

#[cfg(feature = "tasks")]
pub use tasks::{TasksMcp, TasksMcpArgs};

#[cfg(feature = "subagents")]
pub use subagents::{SubAgentsMcp, SubAgentsMcpArgs};

#[cfg(feature = "survey")]
pub use survey::SurveyMcp;

#[cfg(feature = "plan")]
pub use plan::{DEFAULT_PLAN_PROMPT, PlanMcp, PlanMcpArgs};
