#[path = "components/bordered_text_field.rs"]
mod bordered_text_field;
#[path = "components/checkbox.rs"]
mod checkbox;
#[path = "components/multi_select.rs"]
mod multi_select;
#[path = "components/number_field.rs"]
mod number_field;
#[path = "components/radio_select.rs"]
mod radio_select;
#[path = "components/spinner.rs"]
mod spinner;
#[path = "components/split_panel.rs"]
mod split_panel;
#[path = "components/stepper.rs"]
mod stepper;
#[path = "components/text_field.rs"]
mod text_field;

use tui::testing::{
    TestTerminal, assert_buffer_eq, key, render_component, render_component_with_renderer, render_lines, sample_options,
};
use tui::{Component, Event, ViewContext};
