//! Every integration test for wisp-next lives in this one binary: separate
//! test targets each relink the whole crate, which dominates the build.

#[path = "tui/support.rs"]
mod support;

#[path = "tui/attachments.rs"]
mod attachments;
#[path = "tui/composer.rs"]
mod composer;
#[path = "tui/configuration.rs"]
mod configuration;
#[path = "tui/conversation.rs"]
mod conversation;
#[path = "tui/filterable_list.rs"]
mod filterable_list;
#[path = "tui/foundation.rs"]
mod foundation;
#[path = "tui/frame.rs"]
mod frame;
#[path = "tui/git_diff.rs"]
mod git_diff;
#[path = "tui/plan_review.rs"]
mod plan_review;
#[path = "tui/plans.rs"]
mod plans;
#[path = "tui/prompt_search.rs"]
mod prompt_search;
#[path = "tui/scrollback.rs"]
mod scrollback;
#[path = "tui/session_config_view.rs"]
mod session_config_view;
#[path = "tui/sessions.rs"]
mod sessions;
#[path = "tui/settings.rs"]
mod settings;
#[path = "tui/status_line.rs"]
mod status_line;
#[path = "tui/subagents.rs"]
mod subagents;
#[path = "tui/terminal_interaction.rs"]
mod terminal_interaction;
#[path = "tui/terminal_modes.rs"]
mod terminal_modes;
#[path = "tui/workspace.rs"]
mod workspace;
