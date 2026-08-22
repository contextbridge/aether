use super::config::cycle_reasoning_option;
use super::{App, ExitState, Overlay, Route};
use crate::command::{AgentCommand, Command};
use crate::renderer::DrawContext;
use crate::screens::git_diff::GitDiffScreen;
use crate::session::session_config_view::LocalConfigView;
use crate::surfaces::composer::ComposerOutcome;
use crate::surfaces::dropped_files::parse_dropped_file_paths;
use crate::surfaces::input::{MouseAction, RootOutput, UiEvent};
use crate::surfaces::picker::CommandEntry;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use std::time::{Duration, Instant};

pub(super) const CTRL_C_CONFIRM_WINDOW: Duration = Duration::from_secs(1);

impl App {
    pub fn on_terminal_event(&mut self, event: Event) {
        let event = match event {
            Event::Key(key) => UiEvent::Key(key),
            Event::Paste(text) => UiEvent::Paste(text),
            Event::Mouse(mouse) => {
                let Some(action) = MouseAction::from_event(mouse.kind) else { return };
                UiEvent::Mouse(action, (mouse.column, mouse.row))
            }
            Event::Resize(width, _) => {
                self.composer.on_resize(width);
                return;
            }
            _ => return,
        };
        self.on_ui_event(event);
    }

    fn on_ui_event(&mut self, event: UiEvent) {
        let Some(event) = normalize_ui_event(event) else {
            return;
        };
        // Exit handling must precede route and overlay dispatch so no modal can swallow it.
        if let UiEvent::Key(key) = &event
            && self.ui.keybindings.exit.matches(*key)
        {
            self.arm_or_confirm_exit();
            return;
        }

        if let Some(overlay) = self.overlay.as_mut() {
            let actions = match overlay {
                Overlay::Settings(overlay) => {
                    overlay.on_ui_event(event).into_iter().map(RootOutput::Settings).collect()
                }
                Overlay::Sessions(picker) => picker.on_ui_event(event).into_iter().map(RootOutput::Session).collect(),
                Overlay::Workspaces(picker) => {
                    picker.on_ui_event(event).into_iter().map(RootOutput::Workspace).collect()
                }
                Overlay::Elicitation(modal) => {
                    modal.on_ui_event(event).into_iter().map(RootOutput::Elicitation).collect()
                }
            };
            self.dispatch_outputs(actions);
            return;
        }
        let actions: Vec<RootOutput> = match &mut self.route {
            Route::GitReview(screen) => screen.on_ui_event(event).into_iter().map(RootOutput::GitReview).collect(),
            Route::PlanReview(screen) => screen.on_ui_event(event).into_iter().map(RootOutput::PlanReview).collect(),
            Route::Conversation => {
                return match event {
                    UiEvent::Key(key) => self.dispatch_key(key),
                    UiEvent::Paste(text) => self.on_composer_paste(&text),
                    UiEvent::Mouse(action, (_, row)) => self.composer.on_overlay_mouse(action, row),
                };
            }
        };
        self.dispatch_outputs(actions);
    }

    fn arm_or_confirm_exit(&mut self) {
        if self.exit_state.is_confirming() {
            self.exit_state = ExitState::Exiting;
        } else {
            self.composer.clear();
            self.exit_state = ExitState::Confirming(Instant::now());
        }
    }

    /// Routes a keystroke the conversation owns. Routes and overlays are handled by
    /// [`Self::on_ui_event`] before this is reached.
    fn dispatch_key(&mut self, key: KeyEvent) {
        if let Some(outcome) = self.composer.on_prompt_search_key(key) {
            self.apply_composer_outcome(outcome);
            return;
        }

        self.on_composer_key(key);
    }

    /// Acts on the little the composer's overlays cannot do for themselves.
    fn apply_composer_outcome(&mut self, outcome: ComposerOutcome) {
        match outcome {
            ComposerOutcome::Handled => {}
            ComposerOutcome::AcceptedCommand(command) => self.run_accepted_command(&command),
            ComposerOutcome::Search(query) => self.send_prompt_search_query(query),
        }
    }

    fn on_composer_key(&mut self, key: KeyEvent) {
        if self.ui.keybindings.open_prompt_search.matches(key)
            && self.session.capabilities().prompt_search
            && !self.composer.has_completion()
        {
            self.composer.open_prompt_search();
            return;
        }

        if self.ui.keybindings.toggle_git_diff.matches(key) {
            let (screen, task) = GitDiffScreen::new(self.session.working_dir().to_path_buf());
            self.open_route(Route::GitReview(Box::new(screen)));
            self.queue(Command::Git(task));
            return;
        }

        if key.code == KeyCode::Enter && key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT)
            || key.code == KeyCode::Char('j') && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.composer.insert_newline();
            return;
        }

        if let Some(outcome) = self.composer.on_completion_key(key) {
            self.apply_composer_outcome(outcome);
            return;
        }

        if self.ui.keybindings.cycle_reasoning.matches(key) {
            self.apply_config_cycle(cycle_reasoning_option(self.session.config_options()));
            return;
        }

        if self.ui.keybindings.cycle_mode.matches(key) {
            let view = LocalConfigView::new(self.session.config_options());
            let next = view.next_mode().map(|(id, value)| (id.to_string(), value.to_string()));
            self.apply_config_cycle(next);
            return;
        }

        if self.ui.keybindings.submit.matches(key) {
            self.submit();
            return;
        }

        if self.ui.keybindings.cancel.matches(key) {
            if self.waiting_for_response() {
                self.queue(Command::Agent(AgentCommand::Cancel { session_id: self.session.session_id().clone() }));
            }
            return;
        }

        if let KeyCode::Char(character) = key.code {
            let opens_command_picker =
                self.ui.keybindings.open_command_picker.matches(key) && self.composer.text().is_empty();
            if opens_command_picker || self.ui.keybindings.open_file_picker.matches(key) {
                self.composer.insert_char(character);
                if opens_command_picker {
                    self.composer.open_command_picker(self.available_commands.clone());
                } else {
                    let command = self.composer.open_file_picker(self.session.working_dir());
                    self.queue(Command::Filesystem(command));
                }
                return;
            }
        }

        match key.code {
            KeyCode::Char(character) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.composer.insert_char(character);
            }
            // Up/Down fall through to prompt history once the cursor is on the
            // first or last line of the composer.
            KeyCode::Up if !self.composer.move_up() => {
                self.composer.recall_previous();
            }
            KeyCode::Down if !self.composer.move_down() => {
                self.composer.recall_next();
            }
            _ => {
                self.composer.apply_edit_key(key);
            }
        }
    }

    fn apply_config_cycle(&mut self, next: Option<(String, String)>) {
        if let Some((id, value)) = next {
            self.queue(Command::Agent(AgentCommand::SetConfigOption {
                session_id: self.session.session_id().clone(),
                config_id: id.clone(),
                value: value.clone(),
            }));
            self.session.update_config_option_value(&id, &value);
        }
    }

    /// Builtin commands run as soon as they are accepted; commands taking input
    /// leave the composer ready for it.
    fn run_accepted_command(&mut self, command: &CommandEntry) {
        if command.builtin {
            self.dispatch_builtin_command(command);
        } else if command.has_input {
            self.composer.insert_char(' ');
        } else {
            self.submit();
        }
    }

    fn on_composer_paste(&mut self, text: &str) {
        if let Some(outcome) = self.composer.on_prompt_search_paste(text) {
            self.apply_composer_outcome(outcome);
            return;
        }
        let added = parse_dropped_file_paths(text).is_some_and(|paths| self.composer.add_dropped_media(paths));
        if !added {
            self.composer.insert_paste(text);
        }
        self.composer.refresh_overlay_query();
    }

    pub fn render_route(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        match &mut self.route {
            Route::Conversation => None,
            Route::GitReview(screen) => screen.render(area, buf, cx),
            Route::PlanReview(screen) => screen.render(area, buf, cx),
        }
    }

    pub fn render_overlay(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        match self.overlay.as_mut() {
            Some(Overlay::Settings(overlay)) => overlay.render(area, buf, cx),
            Some(Overlay::Sessions(picker)) => picker.render(area, buf, cx),
            Some(Overlay::Workspaces(picker)) => picker.render(area, buf, cx),
            Some(Overlay::Elicitation(modal)) => modal.render(area, buf, cx),
            None => None,
        }
    }

    /// Only the bare composer works without mouse reporting; every other
    /// route or overlay has scrollable or clickable content.
    pub fn needs_mouse_capture(&self) -> bool {
        match self.overlay.as_ref() {
            Some(Overlay::Elicitation(modal)) => modal.needs_mouse_capture(),
            Some(Overlay::Settings(overlay)) => overlay.needs_mouse_capture(),
            Some(_) => true,
            None => match self.route {
                Route::Conversation => self.composer.has_open_overlay(),
                Route::GitReview(_) | Route::PlanReview(_) => true,
            },
        }
    }
}

/// Normalizes terminal key delivery once at the application boundary. Repeats
/// have press semantics, releases never reach feature routing, and all other
/// event kinds pass through unchanged. Features may then apply their own exact
/// versus contained modifier policy to the normalized key.
fn normalize_ui_event(event: UiEvent) -> Option<UiEvent> {
    match event {
        UiEvent::Key(mut key) => match key.kind {
            KeyEventKind::Press | KeyEventKind::Repeat => {
                key.kind = KeyEventKind::Press;
                Some(UiEvent::Key(key))
            }
            KeyEventKind::Release => None,
        },
        event => Some(event),
    }
}
