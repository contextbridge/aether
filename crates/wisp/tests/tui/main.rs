#![allow(clippy::absolute_paths, clippy::manual_contains, clippy::similar_names, clippy::single_char_pattern)]

//! Every integration test for wisp lives in this one binary: separate
//! test targets each relink the whole crate, which dominates the build.

mod support;

mod attachments;
mod canonical_conversation;
mod composer;
mod configuration;
mod conversation;
mod filterable_list;
mod foundation;
mod frame;
mod git_diff;
mod perf;
mod plan_review;
mod plans;
mod progress_indicator;
mod prompt_search;
mod scrollback;
mod session_config_view;
mod sessions;
mod settings;
mod status_line;
mod subagents;
mod terminal_interaction;
mod terminal_session;
mod widgets;
mod workspace;
