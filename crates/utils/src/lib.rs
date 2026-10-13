#![doc = include_str!("../README.md")]

pub mod artifact_review;
pub mod display_meta;
#[cfg(not(target_family = "wasm"))]
pub mod git;
pub mod mcp_status;
pub mod name_pattern;
pub mod panic;
pub mod path;
pub mod reasoning;
pub mod resource_path;
pub mod schema_document;
pub mod settings;
#[cfg(not(target_family = "wasm"))]
pub mod shell_expander;
pub mod substitution;
#[cfg(not(target_family = "wasm"))]
pub mod temp_dir;
#[cfg(all(feature = "testing", not(target_family = "wasm")))]
pub mod testing;
#[cfg(not(target_family = "wasm"))]
pub mod tool_result_truncator;
pub mod variables;

pub use name_pattern::matches_name_pattern;
pub use path::home_relative_path;
pub use reasoning::ReasoningEffort;
pub use resource_path::{PathOrObject, ResourcePath, string_or_object_schema};
pub use settings::SettingsStore;
