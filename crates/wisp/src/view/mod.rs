//! Rendering convention for this module: `Widget` values contain only the
//! short-lived visual configuration for one frame. Persistent selection,
//! scrolling, hit-test geometry, and input drafts stay in controller state and
//! are passed to `StatefulWidget` implementations separately. Line-building
//! helpers remain plain functions because they are also measured, cached, or
//! inserted into scrollback before a terminal area exists.
pub mod diff;
pub(crate) mod edit_buffer;

pub mod filterable_list;
pub mod generation;
pub mod list_view;
pub(crate) mod markdown;
pub(crate) mod reasoning_bar;
pub mod selection;
pub mod syntax;
pub mod widgets;
pub(crate) mod wrap;
