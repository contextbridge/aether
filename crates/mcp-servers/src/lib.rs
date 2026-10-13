#![doc = include_str!("../README.md")]

#[cfg(any(feature = "coding", feature = "subagents"))]
mod background_tasks;

#[cfg(any(feature = "coding", feature = "review"))]
mod request_context;

pub mod workspace_paths;

#[cfg(any(test, feature = "test-helpers"))]
pub mod testing;

#[cfg(feature = "subagents")]
pub mod setup;
#[cfg(feature = "subagents")]
pub use setup::{Builtin, McpBuilderExt, agent_mcp};

#[cfg(feature = "file-ops")]
pub mod file_ops;

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

#[cfg(feature = "review")]
pub mod review;

#[cfg(feature = "coding")]
pub use coding::{CodingMcp, CodingMcpArgs, PermissionMode};

#[cfg(feature = "coding")]
pub use lsp::LspRegistry;

#[cfg(feature = "skills")]
pub use skills::{SkillsMcp, SkillsMcpArgs};

#[cfg(feature = "tasks")]
pub use tasks::{TasksMcp, TasksMcpArgs};

#[cfg(feature = "subagents")]
pub use subagents::{SubAgentsMcp, SubAgentsMcpArgs};

#[cfg(feature = "review")]
pub use review::{
    DocumentSource, HtmlAnnotation, HtmlSource, ReviewArtifactInput, ReviewArtifactOutput, ReviewMcp, ReviewMcpArgs,
};

#[cfg(any(feature = "coding", feature = "skills", feature = "tasks", feature = "review"))]
pub(crate) fn server_config(
    name: &str,
    capabilities: rmcp::model::ServerCapabilities,
    instructions: impl Into<String>,
) -> rmcp::model::ServerConfig {
    rmcp::model::ServerConfig::new(capabilities)
        .with_server_info(rmcp::model::Implementation::new(name, env!("CARGO_PKG_VERSION")))
        .with_instructions(instructions)
}
