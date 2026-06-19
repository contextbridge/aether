#![doc = include_str!("../docs/testing.md")]

mod helpers;
mod test_terminal;

pub use helpers::{
    cols, key, pad, render_component, render_component_with_renderer, render_lines, sample_options, style_at_column,
};
pub use test_terminal::{Cell, TestTerminal, assert_buffer_eq};
