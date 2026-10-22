//! A thread's chat: its transcript, the agent's plan, and the composer that sends the next
//! prompt.

mod code;
mod completion;
mod composer;
mod config_picker;
mod diff;
mod footer;
mod markdown;
mod notice;
mod plan;
mod prompt_search;
mod thinking;
mod tool_card;
mod transcript;

pub(crate) use composer::{Composer, SearchPrompts};
pub(crate) use config_picker::bind_keys as bind_config_picker_keys;
pub(crate) use plan::PlanPanel;
pub(crate) use transcript::Transcript;
