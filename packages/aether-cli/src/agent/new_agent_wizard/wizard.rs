use super::draft_agent_entry::DraftAgentEntry;
use super::new_agent_step::{McpConfigFile, NewAgentMode, NewAgentOutcome, NewAgentStep, PromptFile};
use super::steps::{IdentityStep, ModelStep, PromptsStep, StepCommand, ToolsStep, default_servers};
use crate::error::CliError;
use std::cmp::Ordering;
use std::io::{self, Write};
use std::process::{Command, Stdio};
use tui::{
    CrosstermEvent, Event, Frame, KeyCode, Line, StepVisualState, Stepper, StepperItem, Style, TerminalRuntime,
    ViewContext,
};
use wisp::components::model_selector::ModelEntry;

enum NewAgentAction {
    Done(NewAgentOutcome),
    EditSystemMd,
}

pub struct NewAgentWizard {
    mode: NewAgentMode,
    step: NewAgentStep,
    draft: DraftAgentEntry,
    identity: IdentityStep,
    model: ModelStep,
    prompts: PromptsStep,
    tools: ToolsStep,
    editor_error: Option<String>,
}

impl NewAgentWizard {
    pub fn new(
        mode: NewAgentMode,
        model_entries: Vec<ModelEntry>,
        prompt_options: &[PromptFile],
        mcp_configs: &[McpConfigFile],
    ) -> Self {
        let prompts = PromptsStep::new(prompt_options);
        Self {
            mode,
            step: NewAgentStep::Identity,
            draft: DraftAgentEntry {
                entry: aether_project::AgentConfig {
                    user_invocable: true,
                    agent_invocable: true,
                    prompts: prompt_options.iter().map(|d| aether_project::PromptSource::file(d.filename())).collect(),
                    ..aether_project::AgentConfig::default()
                },
                system_md_content: String::new(),
                system_md_edited: false,
                selected_mcp_servers: default_servers(),
                workspace_mcp_configs: vec![],
            },
            identity: IdentityStep::new(),
            model: ModelStep::new(model_entries),
            prompts,
            tools: ToolsStep::new(mcp_configs),
            editor_error: None,
        }
    }

    pub fn into_draft(self) -> DraftAgentEntry {
        self.draft
    }

    fn sync_draft_from_step(&mut self) {
        match self.step {
            NewAgentStep::Identity => self.identity.sync_to_draft(&mut self.draft),
            NewAgentStep::Model => self.model.sync_to_draft(&mut self.draft),
            NewAgentStep::Prompts => self.prompts.sync_to_draft(&mut self.draft),
            NewAgentStep::Tools => self.tools.sync_to_draft(&mut self.draft),
        }
    }

    fn sync_step_from_draft(&mut self) {
        match self.step {
            NewAgentStep::Identity => self.identity.sync_from_draft(&self.draft),
            NewAgentStep::Model | NewAgentStep::Tools => {}
            NewAgentStep::Prompts => self.prompts.sync_from_draft(&mut self.draft),
        }
    }

    async fn handle_event(&mut self, event: &Event) -> Option<NewAgentAction> {
        if let Event::Key(key) = event {
            if key.code == KeyCode::BackTab {
                return self.advance_back();
            }
            if key.modifiers.is_empty() {
                match key.code {
                    KeyCode::Esc => return Some(NewAgentAction::Done(NewAgentOutcome::Cancelled)),
                    KeyCode::Enter => return self.submit_step(),
                    KeyCode::Tab => return self.focus_next(),
                    _ => {}
                }
            }
        }

        let cmd = match self.step {
            NewAgentStep::Identity => self.identity.handle_event(event).await,
            NewAgentStep::Model => self.model.handle_event(event).await,
            NewAgentStep::Prompts => self.prompts.handle_event(event).await,
            NewAgentStep::Tools => self.tools.handle_event(event).await,
        };

        self.sync_draft_from_step();

        match cmd {
            StepCommand::EditSystemMd => Some(NewAgentAction::EditSystemMd),
            StepCommand::None => None,
        }
    }

    fn submit_step(&mut self) -> Option<NewAgentAction> {
        self.sync_draft_from_step();
        if !self.can_advance() {
            return None;
        }
        match self.step.next() {
            Some(next) => {
                self.step = next;
                self.sync_step_from_draft();
                None
            }
            None => Some(NewAgentAction::Done(NewAgentOutcome::Applied)),
        }
    }

    fn focus_next(&mut self) -> Option<NewAgentAction> {
        match self.step {
            NewAgentStep::Identity if self.identity.focus_next() => {
                self.sync_draft_from_step();
            }
            NewAgentStep::Tools => {
                self.tools.focus_next();
            }
            _ => {}
        }
        None
    }

    fn advance_back(&mut self) -> Option<NewAgentAction> {
        if matches!(self.step, NewAgentStep::Identity) && self.identity.focus_prev() {
            return None;
        }
        if matches!(self.step, NewAgentStep::Tools) && self.tools.focus_prev() {
            return None;
        }
        if let Some(prev) = self.step.prev() {
            self.sync_draft_from_step();
            self.step = prev;
            self.sync_step_from_draft();
            if matches!(self.step, NewAgentStep::Identity) {
                self.identity.focus_last();
            }
        }
        None
    }

    fn can_advance(&self) -> bool {
        match self.step {
            NewAgentStep::Identity => {
                let d = &self.draft.entry;
                let name_ok = !d.name.trim().is_empty();
                let desc_ok = !d.description.trim().is_empty();
                let any_surface = d.user_invocable || d.agent_invocable;
                let scaffold_ok = !matches!(
                    (&self.mode, d.user_invocable, d.agent_invocable),
                    (NewAgentMode::ScaffoldProject, false, true)
                );
                name_ok && desc_ok && any_surface && scaffold_ok
            }
            NewAgentStep::Model => !self.draft.entry.model.is_empty(),
            NewAgentStep::Prompts | NewAgentStep::Tools => true,
        }
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let w = ctx.size.width;
        let h = ctx.size.height;
        let header_h: u16 = 4;
        let footer_h: u16 = 2;
        let body_h = h.saturating_sub(header_h + footer_h);

        if matches!(self.step, NewAgentStep::Model) {
            self.model.update_viewport(body_h as usize);
        }

        let header = self.render_header(ctx).fit_height(header_h, w);
        let footer = self.render_footer(ctx).fit_height(footer_h, w);
        let body = Frame::new(self.render_body(ctx, w)).fit_height(body_h, w);

        Frame::vstack([header, body, footer]).truncate_height(h)
    }

    fn render_header(&self, ctx: &ViewContext) -> Frame {
        let title = match self.mode {
            NewAgentMode::ScaffoldProject => "Create a new Aether project",
            NewAgentMode::AddAgentToExistingProject => "Add a new agent",
        };

        let steps = NewAgentStep::all();
        let current_idx = steps.iter().position(|s| *s == self.step).unwrap_or(0);
        let items: Vec<StepperItem> = steps
            .iter()
            .enumerate()
            .map(|(i, step)| StepperItem {
                label: step.title(),
                state: match i.cmp(&current_idx) {
                    Ordering::Less => StepVisualState::Complete,
                    Ordering::Equal => StepVisualState::Current,
                    Ordering::Greater => StepVisualState::Upcoming,
                },
            })
            .collect();
        let stepper = Stepper { items: &items, separator: "   \u{2500}   ", leading_padding: 2 };

        Frame::new(vec![
            Line::styled(format!("  {title}"), ctx.theme.primary()),
            Line::new(String::new()),
            stepper.render(ctx),
            Line::new(String::new()),
        ])
    }

    fn render_footer(&self, ctx: &ViewContext) -> Frame {
        let forward = if matches!(self.step, NewAgentStep::Tools) { "finish" } else { "next" };
        let show_tab = matches!(self.step, NewAgentStep::Identity)
            || (matches!(self.step, NewAgentStep::Tools) && self.tools.has_multiple_sections());
        let tab_hint = if show_tab { "[tab] field   " } else { "" };
        let reasoning = if matches!(self.step, NewAgentStep::Model) { "   [\u{2190}\u{2192}] reasoning" } else { "" };
        let keys = format!(
            "  [enter] {forward}   {tab_hint}[shift+tab] back   [space] toggle   [\u{2191}\u{2193}] move{reasoning}   [esc] cancel"
        );
        Frame::new(vec![Line::new(String::new()), Line::styled(keys, ctx.theme.muted())])
    }

    fn render_body(&mut self, ctx: &ViewContext, pane_w: u16) -> Vec<Line> {
        let mut lines = Vec::new();
        lines.push(Line::with_style(format!("  {}", self.step.heading()), Style::fg(ctx.theme.heading()).bold()));
        lines.push(Line::new(String::new()));

        match self.step {
            NewAgentStep::Identity => {
                lines.extend(self.identity.render(ctx, pane_w, &self.draft, &self.mode));
            }
            NewAgentStep::Model => {
                lines.extend(self.model.render(ctx));
            }
            NewAgentStep::Prompts => {
                let system_md_path = self.draft.generated_paths(&self.mode).system_md.display().to_string();
                lines.extend(self.prompts.render(ctx, pane_w, &self.draft.system_md_content, &system_md_path));
                if let Some(err) = &self.editor_error {
                    lines.push(Line::styled(format!("  \u{26a0} {err}"), ctx.theme.warning()));
                }
            }
            NewAgentStep::Tools => {
                lines.extend(self.tools.render(ctx));
            }
        }

        lines
    }
}

pub async fn run_wizard_loop<W: io::Write>(
    wizard: &mut NewAgentWizard,
    terminal: &mut TerminalRuntime<W>,
) -> Result<NewAgentOutcome, CliError> {
    terminal.render_frame(|ctx| wizard.render(ctx)).map_err(CliError::IoError)?;

    loop {
        let Some(event) = terminal.next_event().await else {
            return Ok(NewAgentOutcome::Cancelled);
        };
        if let CrosstermEvent::Resize(c, r) = &event {
            terminal.on_resize((*c, *r));
        }
        if let Ok(tui_event) = Event::try_from(event) {
            match wizard.handle_event(&tui_event).await {
                Some(NewAgentAction::Done(outcome)) => return Ok(outcome),
                Some(NewAgentAction::EditSystemMd) => {
                    let editor = std::env::var("VISUAL")
                        .or_else(|_| std::env::var("EDITOR"))
                        .unwrap_or_else(|_| "vi".to_string());
                    edit_system_md(wizard, terminal, &editor).await?;
                }
                None => {}
            }
            terminal.render_frame(|ctx| wizard.render(ctx)).map_err(CliError::IoError)?;
        }
    }
}

async fn edit_system_md<W: io::Write>(
    wizard: &mut NewAgentWizard,
    terminal: &mut TerminalRuntime<W>,
    editor: &str,
) -> Result<(), CliError> {
    let mut tmp =
        tempfile::Builder::new().prefix("aether-system-").suffix(".md").tempfile().map_err(CliError::IoError)?;
    tmp.write_all(wizard.draft.system_md_content.as_bytes()).map_err(CliError::IoError)?;
    tmp.flush().map_err(CliError::IoError)?;
    let path = tmp.path().to_path_buf();

    let mut command = Command::new(editor);
    command.arg(&path).stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit());

    if let Err(err) = terminal.run_external(command).await {
        tracing::warn!(%editor, %err, "failed to open editor");
        wizard.editor_error = Some(format!("Could not open editor '{editor}': {err}"));
        return Ok(());
    }

    wizard.editor_error = None;

    let edited = std::fs::read_to_string(&path).map_err(CliError::IoError)?;
    wizard.draft.system_md_content = edited;
    wizard.draft.system_md_edited = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm::ReasoningEffort;
    use tui::KeyModifiers;
    use wisp::components::model_selector::{ModelEntry, ModelSelector};

    fn make_wizard(mode: NewAgentMode) -> NewAgentWizard {
        NewAgentWizard::new(mode, vec![], &[PromptFile::Agents], &[])
    }

    #[test]
    fn scaffold_mode_blocks_agent_only_exposure() {
        let mut wizard = make_wizard(NewAgentMode::ScaffoldProject);
        wizard.identity.name.inner.value = "Test".to_string();
        wizard.identity.description.inner.value = "Test agent".to_string();
        wizard.draft.entry.name = "Test".to_string();
        wizard.draft.entry.description = "Test agent".to_string();
        wizard.draft.entry.user_invocable = false;
        wizard.draft.entry.agent_invocable = true;
        wizard.step = NewAgentStep::Identity;

        assert!(!wizard.can_advance());
    }

    #[test]
    fn scaffold_mode_allows_user_only_exposure() {
        let mut wizard = make_wizard(NewAgentMode::ScaffoldProject);
        wizard.identity.name.inner.value = "Test".to_string();
        wizard.identity.description.inner.value = "Test agent".to_string();
        wizard.draft.entry.name = "Test".to_string();
        wizard.draft.entry.description = "Test agent".to_string();
        wizard.draft.entry.user_invocable = true;
        wizard.draft.entry.agent_invocable = false;
        wizard.step = NewAgentStep::Identity;

        assert!(wizard.can_advance());
    }

    #[test]
    fn add_agent_mode_allows_agent_only_exposure() {
        let mut wizard = make_wizard(NewAgentMode::AddAgentToExistingProject);
        wizard.identity.name.inner.value = "Test".to_string();
        wizard.identity.description.inner.value = "Test agent".to_string();
        wizard.draft.entry.name = "Test".to_string();
        wizard.draft.entry.description = "Test agent".to_string();
        wizard.draft.entry.user_invocable = false;
        wizard.draft.entry.agent_invocable = true;
        wizard.step = NewAgentStep::Identity;

        assert!(wizard.can_advance());
    }

    #[test]
    fn both_surfaces_unchecked_blocks_advance() {
        let mut wizard = make_wizard(NewAgentMode::AddAgentToExistingProject);
        wizard.identity.name.inner.value = "Test".to_string();
        wizard.identity.description.inner.value = "Test agent".to_string();
        wizard.draft.entry.name = "Test".to_string();
        wizard.draft.entry.description = "Test agent".to_string();
        wizard.draft.entry.user_invocable = false;
        wizard.draft.entry.agent_invocable = false;
        wizard.step = NewAgentStep::Identity;

        assert!(!wizard.can_advance());
    }

    fn key_event(code: KeyCode) -> Event {
        Event::Key(tui::KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: tui::KeyEventKind::Press,
            state: tui::KeyEventState::empty(),
        })
    }

    #[tokio::test]
    async fn identity_typing_only_updates_focused_field() {
        let mut wizard = make_wizard(NewAgentMode::ScaffoldProject);
        wizard.step = NewAgentStep::Identity;

        wizard.handle_event(&key_event(KeyCode::Char('a'))).await;
        assert_eq!(wizard.draft.entry.name, "a");
        assert_eq!(wizard.draft.entry.description, "");

        wizard.handle_event(&key_event(KeyCode::Tab)).await;
        wizard.handle_event(&key_event(KeyCode::Char('b'))).await;
        assert_eq!(wizard.draft.entry.name, "a");
        assert_eq!(wizard.draft.entry.description, "b");
    }

    #[tokio::test]
    async fn identity_tab_cycles_focus_past_text_fields() {
        let mut wizard = make_wizard(NewAgentMode::ScaffoldProject);
        wizard.step = NewAgentStep::Identity;

        wizard.handle_event(&key_event(KeyCode::Tab)).await;
        wizard.handle_event(&key_event(KeyCode::Tab)).await;
        wizard.handle_event(&key_event(KeyCode::Char('x'))).await;

        assert_eq!(wizard.draft.entry.name, "");
        assert_eq!(wizard.draft.entry.description, "");
    }

    #[tokio::test]
    async fn identity_enter_does_not_move_focus() {
        let mut wizard = make_wizard(NewAgentMode::ScaffoldProject);
        wizard.step = NewAgentStep::Identity;

        wizard.handle_event(&key_event(KeyCode::Char('a'))).await;
        wizard.handle_event(&key_event(KeyCode::Enter)).await;
        wizard.handle_event(&key_event(KeyCode::Char('b'))).await;

        assert_eq!(wizard.draft.entry.name, "ab");
        assert_eq!(wizard.draft.entry.description, "");
        assert_eq!(wizard.identity.focus.focused(), 0);
    }

    #[tokio::test]
    async fn identity_enter_on_last_field_advances_step_when_valid() {
        let mut wizard = make_wizard(NewAgentMode::AddAgentToExistingProject);
        wizard.step = NewAgentStep::Identity;
        wizard.identity.name.set_value("Test".to_string());
        wizard.identity.description.set_value("Test agent".to_string());
        wizard.draft.entry.name = "Test".to_string();
        wizard.draft.entry.description = "Test agent".to_string();
        wizard.draft.entry.user_invocable = true;
        wizard.draft.entry.agent_invocable = true;
        wizard.identity.focus.focus(2);

        wizard.handle_event(&key_event(KeyCode::Enter)).await;

        assert_eq!(wizard.step, NewAgentStep::Model);
    }

    #[tokio::test]
    async fn identity_enter_on_last_field_blocks_when_invalid() {
        let mut wizard = make_wizard(NewAgentMode::AddAgentToExistingProject);
        wizard.step = NewAgentStep::Identity;
        wizard.identity.focus.focus(2);

        wizard.handle_event(&key_event(KeyCode::Enter)).await;

        assert_eq!(wizard.step, NewAgentStep::Identity);
    }

    #[tokio::test]
    async fn tab_on_last_identity_field_is_noop() {
        let mut wizard = make_wizard(NewAgentMode::AddAgentToExistingProject);
        wizard.step = NewAgentStep::Identity;
        wizard.identity.name.set_value("Test".to_string());
        wizard.identity.description.set_value("Test agent".to_string());
        wizard.draft.entry.name = "Test".to_string();
        wizard.draft.entry.description = "Test agent".to_string();
        wizard.draft.entry.user_invocable = true;
        wizard.draft.entry.agent_invocable = true;
        wizard.identity.focus.focus(2);

        wizard.handle_event(&key_event(KeyCode::Tab)).await;

        assert_eq!(wizard.identity.focus.focused(), 2);
        assert_eq!(wizard.step, NewAgentStep::Identity);
    }

    #[tokio::test]
    async fn enter_on_first_identity_field_advances_step_when_valid() {
        let mut wizard = make_wizard(NewAgentMode::AddAgentToExistingProject);
        wizard.step = NewAgentStep::Identity;
        wizard.identity.name.set_value("Test".to_string());
        wizard.identity.description.set_value("Test agent".to_string());
        wizard.draft.entry.name = "Test".to_string();
        wizard.draft.entry.description = "Test agent".to_string();
        wizard.draft.entry.user_invocable = true;
        wizard.draft.entry.agent_invocable = true;
        wizard.identity.focus.focus(0);

        wizard.handle_event(&key_event(KeyCode::Enter)).await;

        assert_eq!(wizard.step, NewAgentStep::Model);
    }

    #[tokio::test]
    async fn back_tab_moves_focus_back_within_identity() {
        let mut wizard = make_wizard(NewAgentMode::ScaffoldProject);
        wizard.step = NewAgentStep::Identity;
        wizard.identity.focus.focus(2);

        wizard.handle_event(&key_event(KeyCode::BackTab)).await;

        assert_eq!(wizard.identity.focus.focused(), 1);
        assert_eq!(wizard.step, NewAgentStep::Identity);
    }

    #[tokio::test]
    async fn back_tab_on_first_identity_field_is_noop() {
        let mut wizard = make_wizard(NewAgentMode::ScaffoldProject);
        wizard.step = NewAgentStep::Identity;

        wizard.handle_event(&key_event(KeyCode::BackTab)).await;

        assert_eq!(wizard.identity.focus.focused(), 0);
        assert_eq!(wizard.step, NewAgentStep::Identity);
    }

    #[tokio::test]
    async fn back_tab_from_model_returns_to_identity_last_field() {
        let mut wizard = make_wizard(NewAgentMode::ScaffoldProject);
        wizard.step = NewAgentStep::Model;

        wizard.handle_event(&key_event(KeyCode::BackTab)).await;

        assert_eq!(wizard.step, NewAgentStep::Identity);
        assert_eq!(wizard.identity.focus.focused(), 2);
    }

    fn single_model_entries() -> Vec<ModelEntry> {
        vec![ModelEntry {
            value: "test:model-a".to_string(),
            name: "Test / Model A".to_string(),
            reasoning_levels: vec![],
            supports_image: false,
            supports_audio: false,
            disabled_reason: None,
        }]
    }

    fn reasoning_model_entries() -> Vec<ModelEntry> {
        vec![ModelEntry {
            value: "test:model-r".to_string(),
            name: "Test / Model R".to_string(),
            reasoning_levels: vec![ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High],
            supports_image: false,
            supports_audio: false,
            disabled_reason: None,
        }]
    }

    #[tokio::test]
    async fn model_step_space_toggles_focused_model() {
        let mut wizard =
            NewAgentWizard::new(NewAgentMode::ScaffoldProject, single_model_entries(), &[PromptFile::Agents], &[]);
        wizard.step = NewAgentStep::Model;

        wizard.handle_event(&key_event(KeyCode::Char(' '))).await;

        assert_eq!(wizard.draft.entry.model, "test:model-a");
    }

    #[tokio::test]
    async fn model_step_enter_advances_when_model_selected() {
        let mut wizard =
            NewAgentWizard::new(NewAgentMode::ScaffoldProject, single_model_entries(), &[PromptFile::Agents], &[]);
        wizard.step = NewAgentStep::Model;

        wizard.handle_event(&key_event(KeyCode::Char(' '))).await;
        wizard.handle_event(&key_event(KeyCode::Enter)).await;

        assert_eq!(wizard.step, NewAgentStep::Prompts);
    }

    #[tokio::test]
    async fn model_step_enter_blocks_without_selection() {
        let mut wizard =
            NewAgentWizard::new(NewAgentMode::ScaffoldProject, single_model_entries(), &[PromptFile::Agents], &[]);
        wizard.step = NewAgentStep::Model;

        wizard.handle_event(&key_event(KeyCode::Enter)).await;

        assert_eq!(wizard.step, NewAgentStep::Model);
    }

    #[tokio::test]
    async fn model_step_right_cycles_reasoning_forward() {
        let mut wizard =
            NewAgentWizard::new(NewAgentMode::ScaffoldProject, reasoning_model_entries(), &[PromptFile::Agents], &[]);
        wizard.step = NewAgentStep::Model;

        wizard.handle_event(&key_event(KeyCode::Right)).await;
        assert_eq!(wizard.draft.entry.reasoning_effort, Some(ReasoningEffort::Low));

        wizard.handle_event(&key_event(KeyCode::Right)).await;
        assert_eq!(wizard.draft.entry.reasoning_effort, Some(ReasoningEffort::Medium));

        wizard.handle_event(&key_event(KeyCode::Right)).await;
        assert_eq!(wizard.draft.entry.reasoning_effort, Some(ReasoningEffort::High));

        wizard.handle_event(&key_event(KeyCode::Right)).await;
        assert_eq!(wizard.draft.entry.reasoning_effort, None);
    }

    #[tokio::test]
    async fn model_step_left_cycles_reasoning_backward() {
        let mut wizard =
            NewAgentWizard::new(NewAgentMode::ScaffoldProject, reasoning_model_entries(), &[PromptFile::Agents], &[]);
        wizard.step = NewAgentStep::Model;

        wizard.handle_event(&key_event(KeyCode::Left)).await;
        assert_eq!(wizard.draft.entry.reasoning_effort, Some(ReasoningEffort::High));

        wizard.handle_event(&key_event(KeyCode::Left)).await;
        assert_eq!(wizard.draft.entry.reasoning_effort, Some(ReasoningEffort::Medium));

        wizard.handle_event(&key_event(KeyCode::Left)).await;
        assert_eq!(wizard.draft.entry.reasoning_effort, Some(ReasoningEffort::Low));

        wizard.handle_event(&key_event(KeyCode::Left)).await;
        assert_eq!(wizard.draft.entry.reasoning_effort, None);
    }

    #[tokio::test]
    async fn tools_step_enter_finishes_wizard() {
        let mut wizard = make_wizard(NewAgentMode::AddAgentToExistingProject);
        wizard.step = NewAgentStep::Tools;

        let outcome = wizard.handle_event(&key_event(KeyCode::Enter)).await;

        assert!(matches!(outcome, Some(NewAgentAction::Done(NewAgentOutcome::Applied))));
    }

    #[tokio::test]
    async fn prompts_step_e_key_requests_editor() {
        let mut wizard = make_wizard(NewAgentMode::ScaffoldProject);
        wizard.step = NewAgentStep::Prompts;

        let outcome = wizard.handle_event(&key_event(KeyCode::Char('e'))).await;

        assert!(matches!(outcome, Some(NewAgentAction::EditSystemMd)));
    }

    #[tokio::test]
    async fn prompts_step_seeds_system_md_from_draft() {
        let mut wizard = make_wizard(NewAgentMode::ScaffoldProject);
        wizard.draft.entry.name = "Researcher".to_string();
        wizard.draft.entry.description = "Research agent".to_string();
        wizard.step = NewAgentStep::Prompts;
        wizard.sync_step_from_draft();

        assert!(wizard.draft.system_md_content.starts_with("# Researcher\n"));
        assert!(wizard.draft.system_md_content.contains("Research agent"));
    }

    #[tokio::test]
    async fn prompts_step_preserves_edited_system_md_on_rerender() {
        let mut wizard = make_wizard(NewAgentMode::ScaffoldProject);
        wizard.draft.entry.name = "Researcher".to_string();
        wizard.draft.entry.description = "Research agent".to_string();
        wizard.draft.system_md_content = "# Custom body".to_string();
        wizard.draft.system_md_edited = true;
        wizard.step = NewAgentStep::Prompts;
        wizard.sync_step_from_draft();

        assert_eq!(wizard.draft.system_md_content, "# Custom body");
    }

    #[tokio::test]
    async fn model_selector_syncs_to_draft() {
        let entries = vec![ModelEntry {
            value: "test:model-a".to_string(),
            name: "Test / Model A".to_string(),
            reasoning_levels: vec![ReasoningEffort::Medium, ReasoningEffort::High],
            supports_image: false,
            supports_audio: false,
            disabled_reason: None,
        }];
        let selector = ModelSelector::new(entries, "model".to_string(), Some("test:model-a"), Some("high"));

        let mut wizard = make_wizard(NewAgentMode::ScaffoldProject);
        wizard.step = NewAgentStep::Model;
        wizard.model.selector = selector;

        wizard.sync_draft_from_step();

        assert_eq!(wizard.draft.entry.model, "test:model-a");
        assert_eq!(wizard.draft.entry.reasoning_effort, Some(ReasoningEffort::High));
    }

    fn make_wizard_with_mcp_configs(mode: NewAgentMode) -> NewAgentWizard {
        NewAgentWizard::new(mode, vec![], &[PromptFile::Agents], &[McpConfigFile::McpJson])
    }

    #[tokio::test]
    async fn tools_tab_moves_focus_to_mcp_configs() {
        let mut wizard = make_wizard_with_mcp_configs(NewAgentMode::ScaffoldProject);
        wizard.step = NewAgentStep::Tools;

        assert_eq!(wizard.tools.focus, 0);
        wizard.handle_event(&key_event(KeyCode::Tab)).await;
        assert_eq!(wizard.tools.focus, 1);
    }

    #[tokio::test]
    async fn tools_back_tab_moves_focus_back_from_mcp_configs() {
        let mut wizard = make_wizard_with_mcp_configs(NewAgentMode::ScaffoldProject);
        wizard.step = NewAgentStep::Tools;
        wizard.tools.focus = 1;

        wizard.handle_event(&key_event(KeyCode::BackTab)).await;

        assert_eq!(wizard.tools.focus, 0);
        assert_eq!(wizard.step, NewAgentStep::Tools);
    }

    #[tokio::test]
    async fn tools_back_tab_on_first_section_goes_to_prev_step() {
        let mut wizard = make_wizard_with_mcp_configs(NewAgentMode::ScaffoldProject);
        wizard.step = NewAgentStep::Tools;

        wizard.handle_event(&key_event(KeyCode::BackTab)).await;

        assert_eq!(wizard.step, NewAgentStep::Prompts);
    }

    #[tokio::test]
    async fn tools_tab_without_mcp_configs_is_noop() {
        let mut wizard = make_wizard(NewAgentMode::ScaffoldProject);
        wizard.step = NewAgentStep::Tools;

        wizard.handle_event(&key_event(KeyCode::Tab)).await;

        assert_eq!(wizard.tools.focus, 0);
        assert_eq!(wizard.step, NewAgentStep::Tools);
    }

    #[tokio::test]
    async fn tools_mcp_config_toggle_syncs_to_draft() {
        let mut wizard = make_wizard_with_mcp_configs(NewAgentMode::ScaffoldProject);
        wizard.step = NewAgentStep::Tools;
        wizard.tools.focus = 1;

        wizard.handle_event(&key_event(KeyCode::Char(' '))).await;

        assert!(wizard.draft.workspace_mcp_configs.contains(&"mcp.json".to_string()));
    }

    #[tokio::test]
    async fn edit_system_md_with_missing_editor_sets_error() {
        let (w, h) = (80, 24);
        let mut wizard = {
            let mut wizard = make_wizard(NewAgentMode::ScaffoldProject);
            wizard.step = NewAgentStep::Prompts;
            wizard.draft.system_md_content = "original content".to_string();
            wizard
        };

        let mut terminal = TerminalRuntime::headless(Vec::<u8>::new(), (w, h));
        let result = edit_system_md(&mut wizard, &mut terminal, "__nonexistent_editor__").await;

        assert!(result.is_ok(), "edit_system_md should not return an error");
        assert_eq!(wizard.draft.system_md_content, "original content", "content should be unchanged");
        assert!(!wizard.draft.system_md_edited, "edited flag should remain false");
        assert!(wizard.editor_error.is_some(), "editor_error should be set");
        assert!(wizard.editor_error.as_ref().unwrap().contains("__nonexistent_editor__"));

        let ctx = ViewContext::new((w, h));
        let lines = wizard.render_body(&ctx, w);
        let text: Vec<String> = lines.iter().map(Line::plain_text).collect();
        assert!(
            text.iter().any(|l| l.contains("Could not open editor")),
            "expected editor error in rendered output, got: {text:?}"
        );
    }
}
