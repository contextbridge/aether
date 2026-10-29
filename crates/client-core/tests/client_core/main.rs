//! Every integration test for client-core lives in this one binary: separate
//! test targets each relink the whole crate, which dominates the build.

mod support;

mod attachments;
mod completion_token;
mod connection;
mod elicitation_form;
mod format;
mod git_contract;
mod prompt_history;
mod prompt_search;
mod runtime;
mod session_config_view;
mod session_core;
mod session_model;
