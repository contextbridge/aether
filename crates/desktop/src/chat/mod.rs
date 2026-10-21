//! A thread's chat: its transcript, the agent's plan, and the composer that sends the next
//! prompt.

mod completion;
mod composer;
mod footer;
mod markdown;
mod notice;
mod plan;
mod prompt_search;
mod thinking;
mod tool_card;
mod transcript;

pub(crate) use composer::{Composer, SearchPrompts};
pub(crate) use plan::PlanPanel;
pub(crate) use transcript::Transcript;
