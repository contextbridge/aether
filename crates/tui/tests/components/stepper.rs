use super::*;
use tui::{StepVisualState, Stepper, StepperItem};

fn render_stepper(items: &[StepperItem], separator: &str, leading_padding: usize, width: u16) -> TestTerminal {
    let stepper = Stepper { items, separator, leading_padding };
    render_lines(&[stepper.render(&ViewContext::new((width, 1)))], width, 1)
}

#[test]
fn single_current_step() {
    let items = [StepperItem { label: "One", state: StepVisualState::Current }];
    let term = render_stepper(&items, " - ", 0, 40);
    assert_buffer_eq(&term, &["\u{25c9} One"]);
}

#[test]
fn single_complete_step() {
    let items = [StepperItem { label: "Done", state: StepVisualState::Complete }];
    let term = render_stepper(&items, " - ", 0, 40);
    assert_buffer_eq(&term, &["\u{25cf} Done"]);
}

#[test]
fn single_upcoming_step() {
    let items = [StepperItem { label: "Later", state: StepVisualState::Upcoming }];
    let term = render_stepper(&items, " - ", 0, 40);
    assert_buffer_eq(&term, &["\u{25cb} Later"]);
}

#[test]
fn two_steps_with_separator() {
    let items = [
        StepperItem { label: "A", state: StepVisualState::Complete },
        StepperItem { label: "B", state: StepVisualState::Current },
    ];
    let term = render_stepper(&items, " | ", 0, 40);
    assert_buffer_eq(&term, &["\u{25cf} A | \u{25c9} B"]);
}

#[test]
fn three_steps_full_progression() {
    let items = [
        StepperItem { label: "Identity", state: StepVisualState::Complete },
        StepperItem { label: "Model", state: StepVisualState::Current },
        StepperItem { label: "Tools", state: StepVisualState::Upcoming },
    ];
    let term = render_stepper(&items, " - ", 0, 60);
    assert_buffer_eq(&term, &["\u{25cf} Identity - \u{25c9} Model - \u{25cb} Tools"]);
}

#[test]
fn leading_padding() {
    let items = [StepperItem { label: "A", state: StepVisualState::Current }];
    let term = render_stepper(&items, "", 4, 40);
    assert_buffer_eq(&term, &["    \u{25c9} A"]);
}
