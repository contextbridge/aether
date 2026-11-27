#![allow(clippy::manual_contains, clippy::similar_names, clippy::single_char_pattern)]

//! Every integration test for foxfire lives in this one binary: separate
//! test targets each relink the whole crate, which dominates the build.

mod support;

mod acp_reduction;
mod artifact_review;
mod attachments;
mod composer;
mod configuration;
mod connection;
mod conversation;
mod diff_model;
mod filterable_list;
mod foundation;
mod frame;
mod git_diff;
mod host;
mod perf;
mod permissions;
mod plain_acp;
mod plans;
mod progress_indicator;
mod prompt_search;
mod queued_prompts;
mod review_rendering;
mod scrollback;
mod sessions;
mod settings;
mod status_line;
mod subagents;
mod terminal_interaction;
mod terminal_session;
mod theme;
mod widgets;
mod workspace;
