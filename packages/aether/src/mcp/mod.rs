pub mod auth;
mod client;
pub mod config;
pub mod error;
pub mod manager;
pub mod mcp_builder;
pub mod oauth_integration;
pub mod roots;
pub mod run_mcp_task;
pub mod variables;

pub use config::*;
pub use error::{McpError, Result};
pub use manager::{ElicitationRequest, McpManager, ServerInstructions};
pub use mcp_builder::*;
pub use roots::root_from_path;
pub use variables::{VarError, expand_env_vars};

// Re-export rmcp's Root type for convenience
pub use rmcp::model::Root;
