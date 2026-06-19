use tui::Component;
use tui::Event;
use tui::testing::render_component;
use tui::{KeyCode, KeyEvent, KeyModifiers, Theme, ViewContext, display_width_text};
use wisp::components::command_picker::{CommandEntry, CommandPicker};

const DEFAULT_SIZE: (u16, u16) = (120, 40);

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

async fn type_query(picker: &mut CommandPicker, text: &str) {
    for c in text.chars() {
        let _ = picker.on_event(&Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))).await;
    }
}

fn sample_commands() -> Vec<CommandEntry> {
    vec![
        CommandEntry {
            name: "settings".into(),
            description: "Open settings".into(),
            has_input: false,
            hint: None,
            builtin: true,
        },
        CommandEntry {
            name: "search".into(),
            description: "Search code in the project".into(),
            has_input: true,
            hint: Some("query pattern".into()),
            builtin: false,
        },
        CommandEntry {
            name: "web".into(),
            description: "Browse the web".into(),
            has_input: true,
            hint: Some("url".into()),
            builtin: false,
        },
    ]
}

fn rendered_lines(picker: &mut CommandPicker, width: u16, height: u16) -> Vec<String> {
    let term = render_component(|ctx| picker.render(ctx), width, height);
    let all_lines = term.get_lines();
    all_lines.into_iter().filter(|l| !l.is_empty()).collect()
}

fn selected_text(picker: &mut CommandPicker) -> Option<String> {
    let theme = Theme::default();
    let highlight_bg = theme.highlight_bg();
    let term = render_component(|ctx| picker.render(ctx), DEFAULT_SIZE.0, DEFAULT_SIZE.1);
    let output = term.get_lines();
    // Look at column 2 (after the 2-char `prepend("  ")` from Combobox) to find
    // the row with the highlight background — that's the selected row.
    output.iter().enumerate().find(|(i, _)| term.get_style_at(*i, 2).bg == Some(highlight_bg)).map(|(_, l)| l.clone())
}

#[test]
fn init_shows_all_commands() {
    let mut picker = CommandPicker::new(sample_commands());
    let lines = rendered_lines(&mut picker, DEFAULT_SIZE.0, DEFAULT_SIZE.1);
    assert_eq!(lines.len(), 3);
    assert!(lines.iter().any(|l| l.contains("/settings")));
    assert!(lines.iter().any(|l| l.contains("/search")));
    assert!(lines.iter().any(|l| l.contains("/web")));
}

#[tokio::test]
async fn query_filters_by_name() {
    let mut picker = CommandPicker::new(sample_commands());
    type_query(&mut picker, "settin").await;
    let lines = rendered_lines(&mut picker, DEFAULT_SIZE.0, DEFAULT_SIZE.1);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("/settings"));
}

#[tokio::test]
async fn query_filters_by_description() {
    let mut picker = CommandPicker::new(sample_commands());
    type_query(&mut picker, "browse").await;
    let lines = rendered_lines(&mut picker, DEFAULT_SIZE.0, DEFAULT_SIZE.1);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("/web"));
}

#[tokio::test]
async fn selection_wraps() {
    let mut picker = CommandPicker::new(sample_commands());
    let first = selected_text(&mut picker).unwrap();

    picker.on_event(&Event::Key(key(KeyCode::Up))).await;
    let last = selected_text(&mut picker).unwrap();
    assert_ne!(first, last);

    picker.on_event(&Event::Key(key(KeyCode::Down))).await;
    let back_to_first = selected_text(&mut picker).unwrap();
    assert_eq!(first, back_to_first);
}

#[tokio::test]
async fn selected_command_changes_on_move() {
    let mut picker = CommandPicker::new(sample_commands());
    let first = selected_text(&mut picker).unwrap();
    picker.on_event(&Event::Key(key(KeyCode::Down))).await;
    let second = selected_text(&mut picker).unwrap();
    assert_ne!(first, second);
}

#[test]
fn render_includes_hint_for_commands_with_hint() {
    let mut picker = CommandPicker::new(sample_commands());
    let lines = rendered_lines(&mut picker, DEFAULT_SIZE.0, DEFAULT_SIZE.1);

    assert!(
        lines.iter().any(|l| l.contains("[query pattern]")),
        "Should render hint for search command. Got: {lines:?}",
    );
    assert!(lines.iter().any(|l| l.contains("[url]")), "Should render hint for web command. Got: {lines:?}");
}

#[test]
fn render_omits_hint_brackets_for_commands_without_hint() {
    let mut picker = CommandPicker::new(sample_commands());
    let lines = rendered_lines(&mut picker, DEFAULT_SIZE.0, DEFAULT_SIZE.1);

    let config_line = lines.iter().find(|l| l.contains("/settings")).expect("settings command should be rendered");
    assert!(!config_line.contains("  ["), "Config command should not have hint brackets. Got: {config_line}");
}

#[test]
fn selected_entry_has_highlight_background() {
    let mut picker = CommandPicker::new(sample_commands());
    let ctx = ViewContext::new((80, 24));
    let term = render_component(|c| picker.render(c), 80, 24);
    let output = term.get_lines();
    let row = output.iter().position(|l| l.starts_with("  /")).expect("should render a selected line");

    let style = term.style_of_text(row, "/settings").unwrap();
    assert_eq!(style.bg, Some(ctx.theme.highlight_bg()), "selected entry should have highlight background");
}

#[test]
fn selected_entry_has_text_primary_foreground() {
    let mut picker = CommandPicker::new(sample_commands());
    let ctx = ViewContext::new((80, 24));
    let term = render_component(|c| picker.render(c), 80, 24);
    let output = term.get_lines();
    let row = output.iter().position(|l| l.starts_with("  /")).expect("should render a selected line");

    let style = term.style_of_text(row, "/settings").unwrap();
    assert_eq!(style.fg, Some(ctx.theme.text_primary()), "selected entry should have text_primary foreground");
}

#[test]
fn selected_entry_highlight_fills_full_line_width() {
    let mut picker = CommandPicker::new(sample_commands());
    let term = render_component(|ctx| picker.render(ctx), 30, 24);
    let output = term.get_lines();
    let row = output.iter().position(|l| l.starts_with("  /")).expect("should render a selected line");

    let ctx = ViewContext::new((30, 24));
    let last_col_style = term.get_style_at(row, 29);
    assert_eq!(
        last_col_style.bg,
        Some(ctx.theme.highlight_bg()),
        "selected row should fill the full visible width with highlight background",
    );
}

#[test]
fn non_selected_items_have_multi_span_styling() {
    let mut picker = CommandPicker::new(sample_commands());
    let term = render_component(|c| picker.render(c), DEFAULT_SIZE.0, DEFAULT_SIZE.1);
    let output = term.get_lines();
    // Skip the first (selected) row and find a non-selected command line
    let row = output
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, l)| l.starts_with("  /"))
        .map(|(i, _)| i)
        .expect("should have a non-selected command line");

    // Check that the command name and description have different styles
    let name_style = term.style_of_text(row, "/search").or_else(|| term.style_of_text(row, "/web")).unwrap();
    let desc_style =
        term.style_of_text(row, "Search code").or_else(|| term.style_of_text(row, "Browse the web")).unwrap();
    assert_ne!(name_style, desc_style, "Name and description should have different styles");
}

#[test]
fn descriptions_are_column_aligned() {
    let mut picker = CommandPicker::new(sample_commands());
    let lines = rendered_lines(&mut picker, DEFAULT_SIZE.0, DEFAULT_SIZE.1);

    let command_lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    assert_eq!(command_lines.len(), 3);

    // All descriptions should start at the same display column.
    // Find the display column where the description text begins for each line.
    let desc_positions: Vec<usize> = sample_commands()
        .iter()
        .zip(command_lines.iter())
        .map(|(cmd, line)| {
            let byte_pos = line
                .find(&cmd.description)
                .unwrap_or_else(|| panic!("description '{}' not found in '{}'", cmd.description, line));
            display_width_text(&line[..byte_pos])
        })
        .collect();

    assert!(
        desc_positions.windows(2).all(|w| w[0] == w[1]),
        "Descriptions should start at the same column, but positions are: {desc_positions:?}\nLines: {command_lines:?}",
    );
}

#[test]
fn long_commands_are_truncated_to_terminal_width() {
    let commands = vec![CommandEntry {
        name: "verylongcommandnamethatgoesonandon".into(),
        description:
            "This is a very long description that would normally wrap to multiple lines if we didn't truncate it".into(),
        has_input: false,
        hint: Some("some hint text".into()),
        builtin: false,
    }];

    let terminal_width: u16 = 40;
    let mut picker = CommandPicker::new(commands);
    let lines = rendered_lines(&mut picker, terminal_width, 10);
    let command_line = &lines[0];

    assert_eq!(lines.len(), 1);
    assert!(command_line.ends_with("..."), "Expected truncation, got: {command_line}");

    let width = display_width_text(command_line);
    assert!(
        width <= terminal_width as usize,
        "Line width {width} exceeds terminal width {terminal_width}: {command_line}"
    );
}
