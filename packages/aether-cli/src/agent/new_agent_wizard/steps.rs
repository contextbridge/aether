use super::draft_agent_entry::{DraftAgentEntry, build_system_md};
use super::new_agent_step::{McpConfigFile, NewAgentMode, PromptFile, server_options};
use aether_project::PromptSource;
use tui::{
    BorderedTextField, Color, Component, Event, FocusRing, KeyCode, Line, MultiSelect, Panel, SelectOption, Style,
    ViewContext, render_markdown_result,
};
use wisp::components::model_selector::{ModelEntry, ModelSelector, ModelSelectorMessage};

pub enum StepCommand {
    None,
    EditSystemMd,
}

pub struct IdentityStep {
    pub name: BorderedTextField,
    pub description: BorderedTextField,
    pub exposure: MultiSelect,
    pub focus: FocusRing,
}

impl IdentityStep {
    pub fn new() -> Self {
        Self {
            name: BorderedTextField::new("Name", String::new()),
            description: BorderedTextField::new("Description", String::new()),
            exposure: MultiSelect::new(
                vec![
                    SelectOption {
                        value: "user".to_string(),
                        title: "You (the user)".to_string(),
                        description: Some("Launch this agent yourself from the CLI".to_string()),
                    },
                    SelectOption {
                        value: "agent".to_string(),
                        title: "Other agents".to_string(),
                        description: Some("Other agents can invoke this one as a sub-agent".to_string()),
                    },
                ],
                vec![true, true],
            ),
            focus: FocusRing::new(3).without_wrap(),
        }
    }

    pub fn sync_to_draft(&self, draft: &mut DraftAgentEntry) {
        draft.entry.name = self.name.value().to_string();
        draft.entry.description = self.description.value().to_string();
        draft.entry.user_invocable = self.exposure.selected.first().copied().unwrap_or(false);
        draft.entry.agent_invocable = self.exposure.selected.get(1).copied().unwrap_or(false);
    }

    pub fn sync_from_draft(&mut self, draft: &DraftAgentEntry) {
        self.name.set_value(draft.entry.name.clone());
        self.description.set_value(draft.entry.description.clone());
        self.focus.focus(0);
    }

    pub async fn handle_event(&mut self, event: &Event) -> StepCommand {
        match self.focus.focused() {
            0 => {
                let _ = self.name.on_event(event).await;
            }
            1 => {
                let _ = self.description.on_event(event).await;
            }
            _ => {
                let _ = self.exposure.on_event(event).await;
            }
        }
        StepCommand::None
    }

    pub fn focus_next(&mut self) -> bool {
        if self.focus.focused() + 1 < self.focus.len() {
            self.focus.focus_next();
            return true;
        }
        false
    }

    pub fn focus_prev(&mut self) -> bool {
        if self.focus.focused() > 0 {
            self.focus.focus_prev();
            return true;
        }
        false
    }

    pub fn focus_last(&mut self) {
        let last_idx = self.focus.len().saturating_sub(1);
        self.focus.focus(last_idx);
    }

    pub fn render(
        &mut self,
        ctx: &ViewContext,
        pane_w: u16,
        draft: &DraftAgentEntry,
        mode: &NewAgentMode,
    ) -> Vec<Line> {
        let field_width = pane_field_width(pane_w);
        self.name.set_width(field_width);
        self.description.set_width(field_width);

        let mut lines = Vec::new();
        lines.extend(indent_lines(self.name.render_field(ctx, self.focus.is_focused(0)), 2));
        lines.push(Line::new(String::new()));
        lines.extend(indent_lines(self.description.render_field(ctx, self.focus.is_focused(1)), 2));
        lines.push(Line::new(String::new()));
        lines.push(Line::with_style("  Who can use this agent?".to_string(), Style::fg(ctx.theme.heading()).bold()));
        lines.push(Line::new(String::new()));
        lines.extend(indent_lines(self.exposure.render_field(ctx, true), 2));
        if !draft.entry.user_invocable && !draft.entry.agent_invocable {
            lines.push(Line::new(String::new()));
            lines.push(Line::styled("  \u{26a0} Pick at least one".to_string(), ctx.theme.warning()));
        } else if matches!(mode, NewAgentMode::ScaffoldProject)
            && !draft.entry.user_invocable
            && draft.entry.agent_invocable
        {
            lines.push(Line::new(String::new()));
            lines.push(Line::styled(
                "  \u{26a0} A new project needs at least one user-launchable agent".to_string(),
                ctx.theme.warning(),
            ));
        }
        lines
    }
}

pub struct ModelStep {
    pub selector: ModelSelector,
}

impl ModelStep {
    pub fn new(model_entries: Vec<ModelEntry>) -> Self {
        Self { selector: ModelSelector::new(model_entries, "model".to_string(), None, None) }
    }

    pub fn sync_to_draft(&self, draft: &mut DraftAgentEntry) {
        let selected = self.selector.selected_values();
        if !selected.is_empty() {
            draft.entry.model = selected.iter().cloned().collect::<Vec<_>>().join(",");
        }
        draft.entry.reasoning_effort = self.selector.reasoning_effort();
    }

    pub async fn handle_event(&mut self, event: &Event) -> StepCommand {
        if let Event::Key(key) = event
            && key.modifiers.is_empty()
        {
            match key.code {
                KeyCode::Char(' ') => {
                    self.selector.toggle_focused();
                    return StepCommand::None;
                }
                KeyCode::Right => {
                    self.selector.cycle_reasoning_effort_forward();
                    return StepCommand::None;
                }
                KeyCode::Left => {
                    self.selector.cycle_reasoning_effort_back();
                    return StepCommand::None;
                }
                _ => {}
            }
        }
        if let Some(msgs) = self.selector.on_event(event).await {
            for msg in msgs {
                match msg {
                    ModelSelectorMessage::Done(_) => {}
                }
            }
        }
        StepCommand::None
    }

    pub fn update_viewport(&mut self, height: usize) {
        self.selector.update_viewport(height);
    }

    pub fn render(&mut self, ctx: &ViewContext) -> Vec<Line> {
        self.selector.render(ctx).into_lines()
    }
}

pub struct PromptsStep {
    pub prompt_select: MultiSelect,
}

impl PromptsStep {
    pub fn new(prompt_options: &[PromptFile]) -> Self {
        let options: Vec<SelectOption> = prompt_options
            .iter()
            .map(|d| SelectOption {
                value: d.filename().to_string(),
                title: d.filename().to_string(),
                description: Some(d.description().to_string()),
            })
            .collect();
        let selected = vec![true; options.len()];
        Self { prompt_select: MultiSelect::new(options, selected) }
    }

    pub fn sync_to_draft(&self, draft: &mut DraftAgentEntry) {
        let json = self.prompt_select.to_json();
        draft.entry.prompts = json
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(PromptSource::file)).collect())
            .unwrap_or_default();
    }

    pub fn sync_from_draft(&mut self, draft: &mut DraftAgentEntry) {
        for (i, option) in self.prompt_select.options.iter().enumerate() {
            self.prompt_select.selected[i] =
                draft.entry.prompts.iter().any(|d| d.path() == Some(option.value.as_str()));
        }
        if !draft.system_md_edited {
            draft.system_md_content = build_system_md(draft);
        }
    }

    pub async fn handle_event(&mut self, event: &Event) -> StepCommand {
        if let Event::Key(key) = event
            && key.modifiers.is_empty()
            && key.code == KeyCode::Char('e')
        {
            return StepCommand::EditSystemMd;
        }
        let _ = self.prompt_select.on_event(event).await;
        StepCommand::None
    }

    pub fn render(
        &mut self,
        ctx: &ViewContext,
        pane_w: u16,
        system_md_content: &str,
        system_md_path: &str,
    ) -> Vec<Line> {
        let mut lines = Vec::new();

        lines.push(Line::styled(format!("  {system_md_path}"), ctx.theme.text_secondary()));
        lines.push(Line::new(String::new()));

        let inner_w = pane_w.saturating_sub(4);
        let panel_ctx = ctx.with_width(inner_w);
        let mut md_lines = render_markdown_result(system_md_content, &panel_ctx).to_lines();
        md_lines.truncate(10);
        let mut panel = Panel::new(Color::Grey);
        panel.push(md_lines);
        let panel_frame = panel.render(&panel_ctx);
        lines.extend(indent_lines(panel_frame.into_lines(), 2));

        lines.push(Line::new(String::new()));
        lines.push(Line::styled("  [e] edit", ctx.theme.muted()));
        lines.push(Line::new(String::new()));

        if self.prompt_select.options.is_empty() {
            lines.push(Line::styled("  No prompt files detected".to_string(), ctx.theme.muted()));
        } else {
            lines.push(Line::with_style(
                "  Include additional prompt files".to_string(),
                Style::fg(ctx.theme.heading()).bold(),
            ));
            lines.push(Line::new(String::new()));
            lines.extend(indent_lines(self.prompt_select.render_field(ctx, true), 2));
        }

        lines
    }
}

pub struct ToolsStep {
    pub server_select: MultiSelect,
    pub mcp_config_select: Option<MultiSelect>,
    pub focus: usize,
}

impl ToolsStep {
    pub fn new(mcp_configs: &[McpConfigFile]) -> Self {
        let options = server_options();
        let selected = vec![true; options.len()];

        let mcp_config_select = if mcp_configs.is_empty() {
            None
        } else {
            let config_options: Vec<SelectOption> = mcp_configs
                .iter()
                .map(|c| SelectOption {
                    value: c.filename().to_string(),
                    title: c.filename().to_string(),
                    description: Some(c.description().to_string()),
                })
                .collect();
            let config_selected = vec![false; config_options.len()];
            Some(MultiSelect::new(config_options, config_selected))
        };

        Self { server_select: MultiSelect::new(options, selected), mcp_config_select, focus: 0 }
    }

    pub fn sync_to_draft(&self, draft: &mut DraftAgentEntry) {
        let json = self.server_select.to_json();
        draft.selected_mcp_servers = json
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        draft.workspace_mcp_configs = self
            .mcp_config_select
            .as_ref()
            .map(|select| {
                let json = select.to_json();
                json.as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
    }

    pub fn focus_next(&mut self) -> bool {
        if self.focus == 0 && self.mcp_config_select.is_some() {
            self.focus = 1;
            return true;
        }
        false
    }

    pub fn focus_prev(&mut self) -> bool {
        if self.focus > 0 {
            self.focus -= 1;
            return true;
        }
        false
    }

    pub fn has_multiple_sections(&self) -> bool {
        self.mcp_config_select.is_some()
    }

    pub async fn handle_event(&mut self, event: &Event) -> StepCommand {
        match self.focus {
            0 => {
                let _ = self.server_select.on_event(event).await;
            }
            _ => {
                if let Some(ref mut select) = self.mcp_config_select {
                    let _ = select.on_event(event).await;
                }
            }
        }
        StepCommand::None
    }

    pub fn render(&mut self, ctx: &ViewContext) -> Vec<Line> {
        let mut lines = indent_lines(self.server_select.render_field(ctx, self.focus == 0), 2);

        if let Some(ref mut select) = self.mcp_config_select {
            lines.push(Line::new(String::new()));
            lines.push(Line::with_style(
                "  Include workspace MCP configurations".to_string(),
                Style::fg(ctx.theme.heading()).bold(),
            ));
            lines.push(Line::new(String::new()));
            lines.extend(indent_lines(select.render_field(ctx, self.focus == 1), 2));
        }

        lines
    }
}

pub fn default_servers() -> Vec<String> {
    server_options().iter().map(|o| o.value.clone()).collect()
}

fn pane_field_width(pane_w: u16) -> usize {
    (pane_w as usize).saturating_sub(4)
}

fn indent_lines(lines: Vec<Line>, spaces: usize) -> Vec<Line> {
    let prefix = " ".repeat(spaces);
    lines.into_iter().map(|l| l.prepend(prefix.clone())).collect()
}
