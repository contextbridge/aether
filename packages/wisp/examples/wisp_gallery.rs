use std::time::Duration;

use agent_client_protocol::schema::{PlanEntry, PlanEntryPriority, PlanEntryStatus};
use tui::{
    Component, CrosstermEvent, Event, Frame, Gallery, GalleryMessage, Line, MouseCapture, TerminalConfig,
    TerminalRuntime, Theme, ViewContext, terminal_size,
};
use wisp::components::app::{PlanReviewInput, PlanReviewMode};
use wisp::components::command_picker::{CommandEntry, CommandPicker};
use wisp::components::file_picker::{FileMatch, FilePicker};
use wisp::components::plan_review::PlanDocument;
use wisp::components::plan_view::PlanView;
use wisp::components::progress_indicator::ProgressIndicator;
use wisp::components::status_line::{ContextUsageDisplay, StatusLine};
use wisp::components::text_input::TextInput;
use wisp::components::thought_message::ThoughtMessage;
use wisp::components::tool_call_status_view::{ToolCallStatus, ToolCallStatusView};
use wisp::keybindings::Keybindings;
use wisp::workspace_status::WorkspaceStatus;

enum WispStory {
    TextInput(TextInput),
    CommandPicker(CommandPicker),
    FilePicker(FilePicker),
    ProgressIndicator(ProgressIndicatorStory),
    ToolCallStatus(ToolCallStatusStory),
    ThoughtMessage(ThoughtMessageStory),
    StatusLine(StatusLineStory),
    PlanView(PlanViewStory),
    PlanReview(Box<PlanReviewStory>),
}

impl Component for WispStory {
    type Message = ();

    async fn on_event(&mut self, event: &Event) -> Option<Vec<()>> {
        match self {
            Self::TextInput(c) => c.on_event(event).await.map(|_| vec![]),
            Self::CommandPicker(c) => c.on_event(event).await.map(|_| vec![]),
            Self::FilePicker(c) => c.on_event(event).await.map(|_| vec![]),
            Self::ProgressIndicator(c) => c.on_event(event).await,
            Self::ToolCallStatus(c) => c.on_event(event).await,
            Self::ThoughtMessage(c) => c.on_event(event).await,
            Self::StatusLine(c) => c.on_event(event).await,
            Self::PlanView(c) => c.on_event(event).await,
            Self::PlanReview(c) => c.on_event(event).await,
        }
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        match self {
            Self::TextInput(c) => c.render(ctx),
            Self::CommandPicker(c) => c.render(ctx),
            Self::FilePicker(c) => c.render(ctx),
            Self::ProgressIndicator(c) => c.render(ctx),
            Self::ToolCallStatus(c) => c.render(ctx),
            Self::ThoughtMessage(c) => c.render(ctx),
            Self::StatusLine(c) => c.render(ctx),
            Self::PlanView(c) => c.render(ctx),
            Self::PlanReview(c) => c.render(ctx),
        }
    }
}

struct ProgressIndicatorStory {
    indicator: ProgressIndicator,
}

impl Component for ProgressIndicatorStory {
    type Message = ();

    async fn on_event(&mut self, event: &Event) -> Option<Vec<()>> {
        if let Event::Tick = event {
            self.indicator.on_tick();
        }
        Some(vec![])
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        self.indicator.render(ctx)
    }
}

struct ToolCallStatusStory {
    tick: u16,
}

impl Component for ToolCallStatusStory {
    type Message = ();

    async fn on_event(&mut self, event: &Event) -> Option<Vec<()>> {
        if let Event::Tick = event {
            self.tick = self.tick.wrapping_add(1);
        }
        Some(vec![])
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let running = ToolCallStatus::Running;
        let success = ToolCallStatus::Success;
        let error = ToolCallStatus::Error("exit code 1".into());

        let mut lines = Vec::new();

        lines.extend(
            ToolCallStatusView {
                name: "Read",
                arguments: "src/main.rs",
                display_value: None,
                diff_preview: None,
                status: &running,
                tick: self.tick,
            }
            .render(ctx)
            .into_lines(),
        );
        lines.push(Line::default());

        lines.extend(
            ToolCallStatusView {
                name: "Write",
                arguments: "src/lib.rs",
                display_value: Some("src/lib.rs"),
                diff_preview: None,
                status: &success,
                tick: self.tick,
            }
            .render(ctx)
            .into_lines(),
        );
        lines.push(Line::default());

        lines.extend(
            ToolCallStatusView {
                name: "Bash",
                arguments: "cargo test",
                display_value: None,
                diff_preview: None,
                status: &error,
                tick: self.tick,
            }
            .render(ctx)
            .into_lines(),
        );

        Frame::new(lines)
    }
}

struct ThoughtMessageStory {
    text: String,
}

impl Component for ThoughtMessageStory {
    type Message = ();

    async fn on_event(&mut self, _event: &Event) -> Option<Vec<()>> {
        Some(vec![])
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let view = ThoughtMessage { text: &self.text };
        view.render(ctx)
    }
}

struct StatusLineStory;

impl Component for StatusLineStory {
    type Message = ();

    async fn on_event(&mut self, _event: &Event) -> Option<Vec<()>> {
        Some(vec![])
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let workspace_status = WorkspaceStatus::new("~/code/foo", Some("main".to_string()));
        let status = StatusLine {
            workspace_status: &workspace_status,
            agent_name: "aether",
            config_options: &[],
            context_usage: Some(ContextUsageDisplay::new(144_000, 200_000)),
            waiting_for_response: false,
            unhealthy_server_count: 0,
            content_padding: wisp::settings::DEFAULT_CONTENT_PADDING,
            exit_confirmation_active: false,
        };
        status.render(ctx)
    }
}

struct PlanViewStory {
    entries: Vec<PlanEntry>,
}

impl Component for PlanViewStory {
    type Message = ();

    async fn on_event(&mut self, _event: &Event) -> Option<Vec<()>> {
        Some(vec![])
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let view = PlanView { entries: &self.entries };
        view.render(ctx)
    }
}

struct PlanReviewStory {
    mode: PlanReviewMode,
}

impl Component for PlanReviewStory {
    type Message = ();

    async fn on_event(&mut self, event: &Event) -> Option<Vec<()>> {
        self.mode.on_event(event).await;
        Some(vec![])
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        self.mode.render(ctx)
    }
}

fn sample_plan_review_markdown() -> &'static str {
    r"# Overview
We need to add a native markdown review surface in Wisp for plan approval.
The experience should feel like the git diff review flow, but operate on stable source lines from the markdown plan.

## Goals
- Keep the default path inside Wisp.
- Preserve a clean fallback for non-Wisp clients.
- Make the outline pane useful for large plans.

### Success criteria
1. Reviewers can move around with vim-style keys.
2. Reviewers can leave inline comments on source lines.
3. Reviewers can approve, request changes, or cancel.

## UX sketch
The screen should have two panes:
- an outline on the left
- the full markdown source on the right
- inline comments rendered directly below the anchored line

### Interaction details
- `j` / `k` move within the focused pane
- `h` / `l` switch focus
- `Enter` jumps from the outline to the selected section
- `c` creates an inline comment draft
- `a` approves the plan
- `r` requests changes

## Data model
We should parse the markdown into a line-stable structure.

```rust
pub struct PlanDocument {
    pub path: String,
    pub lines: Vec<PlanSourceLine>,
    pub outline: Vec<PlanSection>,
}

pub struct PlanSection {
    pub title: String,
    pub level: u8,
    pub first_line_no: usize,
}
```

### Notes on rendering
Paragraphs can be styled, but comments must stay anchored to the original source line.
That means we should not reflow the markdown before attaching review comments.

## Implementation phases
### Phase 1
- parse headings
- render the outline
- render line numbers
- support basic navigation

### Phase 2
- add inline comments
- compile review feedback
- support approve / deny / cancel

### Phase 3
- reuse shared review primitives with git diff
- reduce duplicate splice and highlight code
- add a gallery story for rapid iteration

## Risks
- The outline and document cursor can fight each other if they both own selection.
- Reflowed markdown would make line comments ambiguous.
- Duplicated rendering logic can drift over time.

## Open questions
- Should long code fences soft-wrap or hard-truncate?
- Should we show section context in compiled feedback?
- Do we want richer markdown styling for tables later?
"
}

fn sample_commands() -> Vec<CommandEntry> {
    vec![
        CommandEntry {
            name: "clear".into(),
            description: "Clear screen and start a new session".into(),
            has_input: false,
            hint: None,
            builtin: true,
        },
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
            hint: Some("query".into()),
            builtin: false,
        },
        CommandEntry {
            name: "web".into(),
            description: "Browse the web".into(),
            has_input: true,
            hint: Some("url".into()),
            builtin: false,
        },
        CommandEntry {
            name: "resume".into(),
            description: "Resume a previous session".into(),
            has_input: false,
            hint: None,
            builtin: true,
        },
    ]
}

fn sample_plan_entries() -> Vec<PlanEntry> {
    vec![
        PlanEntry::new("Research AI agent patterns", PlanEntryPriority::Medium, PlanEntryStatus::Completed),
        PlanEntry::new("Implement task tracking", PlanEntryPriority::High, PlanEntryStatus::Completed),
        PlanEntry::new("Build component gallery", PlanEntryPriority::Medium, PlanEntryStatus::InProgress),
        PlanEntry::new("Write integration tests", PlanEntryPriority::Medium, PlanEntryStatus::Pending),
        PlanEntry::new("Update documentation", PlanEntryPriority::Low, PlanEntryStatus::Pending),
    ]
}

fn sample_files() -> Vec<FileMatch> {
    vec![
        FileMatch { path: "src/main.rs".into(), display_name: "src/main.rs".into() },
        FileMatch { path: "src/lib.rs".into(), display_name: "src/lib.rs".into() },
        FileMatch { path: "src/components/mod.rs".into(), display_name: "src/components/mod.rs".into() },
        FileMatch { path: "src/components/gallery.rs".into(), display_name: "src/components/gallery.rs".into() },
        FileMatch { path: "src/rendering/renderer.rs".into(), display_name: "src/rendering/renderer.rs".into() },
        FileMatch { path: "Cargo.toml".into(), display_name: "Cargo.toml".into() },
        FileMatch { path: "README.md".into(), display_name: "README.md".into() },
    ]
}

fn stories() -> Vec<(String, WispStory)> {
    vec![
        (
            "TextInput".into(),
            WispStory::TextInput(TextInput::new(Keybindings::default())),
        ),
        (
            "CommandPicker".into(),
            WispStory::CommandPicker(CommandPicker::new(sample_commands())),
        ),
        (
            "FilePicker".into(),
            WispStory::FilePicker(FilePicker::new_with_entries(sample_files())),
        ),
        (
            "ProgressIndicator".into(),
            WispStory::ProgressIndicator({
                let mut indicator = ProgressIndicator::default();
                indicator.update(1, 3, true);
                ProgressIndicatorStory { indicator }
            }),
        ),
        (
            "ToolCallStatus".into(),
            WispStory::ToolCallStatus(ToolCallStatusStory { tick: 0 }),
        ),
        (
            "ThoughtMessage".into(),
            WispStory::ThoughtMessage(ThoughtMessageStory {
                text: "Let me analyze the codebase structure.\nI see several modules that need refactoring.\nStarting with the authentication layer."
                    .into(),
            }),
        ),
        ("StatusLine".into(), WispStory::StatusLine(StatusLineStory)),
        (
            "PlanView".into(),
            WispStory::PlanView(PlanViewStory {
                entries: sample_plan_entries(),
            }),
        ),
        (
            "PlanReview".into(),
            WispStory::PlanReview(Box::new(PlanReviewStory {
                mode: PlanReviewMode::new(PlanReviewInput {
                    title: "Review /tmp/gallery-plan.md".to_string(),
                    document: PlanDocument::parse("/tmp/gallery-plan.md", sample_plan_review_markdown()),
                }),
            })),
        ),
    ]
}

#[tokio::main]
async fn main() {
    let mut gallery = Gallery::new(stories());
    let size = terminal_size().unwrap_or((80, 24));
    let mut terminal = TerminalRuntime::new(
        std::io::stdout(),
        Theme::default(),
        size,
        TerminalConfig { bracketed_paste: true, mouse_capture: MouseCapture::Disabled },
    )
    .unwrap();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    terminal.render_frame(|ctx| gallery.render(ctx)).unwrap();

    loop {
        tokio::select! {
            Some(raw) = terminal.next_event() => {
                if let CrosstermEvent::Resize(cols, rows) = &raw {
                    terminal.on_resize((*cols, *rows));
                }
                if let Ok(event) = Event::try_from(raw) {
                    if let Some(msgs) = gallery.on_event(&event).await
                        && msgs.iter().any(|m| matches!(m, GalleryMessage::Quit))
                    {
                        return;
                    }
                    terminal.render_frame(|ctx| gallery.render(ctx)).unwrap();
                }
            }
            _ = tick.tick() => {
                gallery.on_event(&Event::Tick).await;
                terminal.render_frame(|ctx| gallery.render(ctx)).unwrap();
            }
        }
    }
}
