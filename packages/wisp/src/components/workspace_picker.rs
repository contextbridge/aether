use crate::components::common::{INPUT_BOX_INDENT, input_box_frame, input_box_width};
use crate::workspace_status::home_relative_path;
use acp_utils::notifications::{WorkspaceEntry, WorkspaceMoveTarget};
use std::path::{Path, PathBuf};
use tui::{
    BorderedTextField, Combobox, Component, Event, Frame, KeyCode, Line, MouseEventKind, PickerMessage, Searchable,
    Style, ViewContext, truncate_text,
};

pub struct WorkspacePicker {
    rows: Vec<WorkspaceRow>,
    parent_dir: Option<PathBuf>,
    mode: Mode,
}

pub enum WorkspacePickerMessage {
    Close,
    Move { target: WorkspaceMoveTarget },
}

impl WorkspacePicker {
    pub fn new(workspaces: Vec<WorkspaceEntry>) -> Self {
        let parent_dir = workspaces.iter().find(|w| w.is_current).and_then(|w| w.path.parent().map(Path::to_path_buf));
        let mut rows: Vec<WorkspaceRow> =
            workspaces.into_iter().filter(|w| !w.is_current).map(WorkspaceRow::Existing).collect();
        rows.push(WorkspaceRow::CreateNew);
        let mode = Mode::List(Box::new(list_mode(&rows)));
        Self { rows, parent_dir, mode }
    }
}

impl Component for WorkspacePicker {
    type Message = WorkspacePickerMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        match &mut self.mode {
            Mode::List(combobox) => {
                if let Event::Mouse(mouse) = event {
                    match mouse.kind {
                        MouseEventKind::ScrollUp => combobox.move_up(),
                        MouseEventKind::ScrollDown => combobox.move_down(),
                        MouseEventKind::Down(_) => {
                            if let Some(index) = mouse.row.checked_sub(4).map(usize::from) {
                                combobox.set_selected_index(index);
                                if let Some(row) = combobox.selected().cloned() {
                                    return Some(confirm_workspace_row(row));
                                }
                            }
                        }
                        _ => {}
                    }
                    return Some(vec![]);
                }

                let msgs = combobox.handle_picker_event(event)?;
                let mut out = Vec::new();
                let mut start_naming = false;
                for msg in msgs {
                    match msg {
                        PickerMessage::Close | PickerMessage::CloseAndPopChar => {
                            out.push(WorkspacePickerMessage::Close);
                        }
                        PickerMessage::Confirm(WorkspaceRow::Existing(entry)) => {
                            out.push(WorkspacePickerMessage::Move {
                                target: WorkspaceMoveTarget::Existing { path: entry.path },
                            });
                        }
                        PickerMessage::Confirm(WorkspaceRow::CreateNew) => start_naming = true,
                        _ => {}
                    }
                }
                if start_naming {
                    self.mode = Mode::NamingNew {
                        field: BorderedTextField::new("New workspace name", String::new())
                            .placeholder("directory name for the new workspace"),
                    };
                }
                Some(out)
            }
            Mode::NamingNew { field } => {
                let Event::Key(key) = event else {
                    return Some(vec![]);
                };
                match key.code {
                    KeyCode::Esc => {
                        self.mode = Mode::List(Box::new(list_mode(&self.rows)));
                        Some(vec![])
                    }
                    KeyCode::Enter => {
                        let trimmed = field.value().trim();
                        if trimmed.is_empty() {
                            Some(vec![])
                        } else {
                            Some(vec![WorkspacePickerMessage::Move {
                                target: WorkspaceMoveTarget::New { name: trimmed.to_string() },
                            }])
                        }
                    }
                    _ => {
                        field.on_event(event).await;
                        Some(vec![])
                    }
                }
            }
        }
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        match &mut self.mode {
            Mode::List(combobox) => render_list(combobox, context),
            Mode::NamingNew { field } => render_name_input(field, self.parent_dir.as_deref(), context),
        }
    }
}

const CREATE_NEW_LABEL: &str = "Create new workspace…";

enum Mode {
    List(Box<Combobox<WorkspaceRow>>),
    NamingNew { field: BorderedTextField },
}

#[derive(Clone)]
enum WorkspaceRow {
    Existing(WorkspaceEntry),
    CreateNew,
}

impl Searchable for WorkspaceRow {
    fn search_text(&self) -> String {
        match self {
            WorkspaceRow::Existing(entry) => home_relative_path(&entry.path),
            WorkspaceRow::CreateNew => CREATE_NEW_LABEL.to_string(),
        }
    }
}

fn confirm_workspace_row(row: WorkspaceRow) -> Vec<WorkspacePickerMessage> {
    match row {
        WorkspaceRow::Existing(entry) => {
            vec![WorkspacePickerMessage::Move { target: WorkspaceMoveTarget::Existing { path: entry.path } }]
        }
        WorkspaceRow::CreateNew => Vec::new(),
    }
}

fn list_mode(rows: &[WorkspaceRow]) -> Combobox<WorkspaceRow> {
    Combobox::new(rows.to_vec()).close_on_whitespace(false)
}

fn render_list(combobox: &mut Combobox<WorkspaceRow>, context: &ViewContext) -> Frame {
    let search = input_box_frame("Move to workspace", combobox.query(), "type to filter workspaces", context);

    let mut lines = vec![Line::new(String::new())];
    if combobox.is_empty() {
        lines.push(Line::new("  (no matching workspaces)"));
    } else {
        lines.extend(combobox.render_items(context, |row, is_selected, ctx| {
            let text = match row {
                WorkspaceRow::Existing(entry) => format!("  {}", home_relative_path(&entry.path)),
                WorkspaceRow::CreateNew => format!("  {CREATE_NEW_LABEL}"),
            };
            let truncated = truncate_text(&text, ctx.size.width as usize);
            if is_selected {
                ctx.theme.selected_row_line(truncated)
            } else if matches!(row, WorkspaceRow::CreateNew) {
                Line::with_style(truncated, Style::fg(ctx.theme.info()))
            } else {
                Line::new(truncated)
            }
        }));
    }

    Frame::vstack([search, Frame::new(lines)])
}

fn render_name_input(field: &mut BorderedTextField, parent_dir: Option<&Path>, context: &ViewContext) -> Frame {
    let width = input_box_width(context);
    field.set_width(width);
    let input = Frame::new(field.render_field(context, true)).indent(INPUT_BOX_INDENT);
    let mut hint_lines = Vec::new();
    if let Some(parent) = parent_dir {
        let mut hint = Line::new("  ");
        hint.push_with_style(
            format!("will be created in {}/", home_relative_path(parent)),
            Style::fg(context.theme.muted()),
        );
        hint_lines.push(hint);
    }
    Frame::vstack([input, Frame::new(hint_lines)])
}
