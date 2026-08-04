use super::config::{cycle_quick_option, cycle_reasoning_option, update_config_option_value};
use super::{App, ExitState, Layer};
use crate::renderer::DrawContext;
use crate::screens::git_diff::GitDiffScreen;
use crate::session::tasks::TaskResult;
use crate::surfaces::composer::ComposerOutcome;
use crate::surfaces::dropped_files::parse_dropped_file_paths;
use crate::surfaces::picker::CommandEntry;
use crate::surfaces::surface::{MouseAction, Surface, UiEvent};
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
            _ => return,
        };
        self.on_ui_event(event);
    }

    fn on_ui_event(&mut self, event: UiEvent) {
        // Exit handling must precede layer routing so no modal can swallow it.
        if let UiEvent::Key(key) = &event
            && matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
            && self.ui.keybindings.exit.matches(*key)
        {
            self.arm_or_confirm_exit();
            self.refresh_progress();
            return;
        }

        if let Some(layer) = self.layer.as_mut() {
            let actions = layer.surface().on_ui_event(event);
            self.dispatch_actions(actions);
            self.refresh_progress();
            return;
        }
        match event {
            UiEvent::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                self.dispatch_key(key);
            }
            UiEvent::Key(_) => {}
            UiEvent::Paste(text) => self.on_composer_paste(&text),
            UiEvent::Mouse(action, (_, row)) => self.composer.on_overlay_mouse(action, row),
        }
        self.refresh_progress();
    }

    fn arm_or_confirm_exit(&mut self) {
        if self.exit_state.is_confirming() {
            self.exit_state = ExitState::Exiting;
        } else {
            self.composer.clear();
            self.exit_state = ExitState::Confirming(Instant::now());
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        self.on_ui_event(UiEvent::Key(key));
    }

    /// Routes a keystroke the conversation owns. Layers are handled by
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
            && self.agent.capabilities.prompt_search
            && !self.composer.has_completion()
        {
            self.composer.open_prompt_search();
            return;
        }

        if self.ui.keybindings.toggle_git_diff.matches(key) {
            self.open_git_diff();
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
            self.apply_config_cycle(cycle_reasoning_option(&self.agent.config_options));
            return;
        }

        if self.ui.keybindings.cycle_mode.matches(key) {
            self.apply_config_cycle(cycle_quick_option(&self.agent.config_options));
            return;
        }

        if self.ui.keybindings.submit.matches(key) {
            self.submit();
            return;
        }

        if self.ui.keybindings.cancel.matches(key) {
            if self.turn.prompt_in_flight {
                let _ = self.agent.handle.cancel(&self.agent.session_id);
            }
            return;
        }

        match key.code {
            KeyCode::Char(character) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.composer.insert_char(character);
                if self.ui.keybindings.open_command_picker.matches(key) && self.composer.text() == "/" {
                    self.composer.open_command_picker(self.available_commands.clone());
                } else if self.ui.keybindings.open_file_picker.matches(key) {
                    let task = self.composer.open_file_picker(&self.agent.working_dir);
                    self.spawn(task);
                }
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
        if let Some((id, value)) = next
            && self.agent.handle.set_config_option(&self.agent.session_id, &id, &value).is_ok()
        {
            update_config_option_value(&mut self.agent.config_options, &id, &value);
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

    pub fn on_paste(&mut self, text: &str) {
        self.on_ui_event(UiEvent::Paste(text.to_string()));
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

    /// Draws the open layer over the conversation, returning where it wants the
    /// terminal cursor.
    pub fn render_layer(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        self.layer.as_mut()?.render(area, buf, cx)
    }

    /// Routes a completed task to its single owner.
    pub fn on_task_result(&mut self, result: TaskResult) {
        match result {
            TaskResult::FilesIndexed { request_id, files } => self.composer.on_files_indexed(request_id, files),
            TaskResult::SubmissionPrepared(outcome) => self.finish_submission(outcome),
            TaskResult::ThemesListed(files) => self.refresh_settings_themes(&files),
            TaskResult::ThemeApplied { settings, theme, error } => self.finish_theme_change(settings, theme, error),
            TaskResult::WorkspaceResolved { cwd, status } => self.finish_workspace_move(&cwd, status),
            result @ TaskResult::GitDiff(_) => {
                let Some(layer) = self.layer.as_mut() else { return };
                let actions = layer.surface().on_task_result(result);
                self.dispatch_actions(actions);
            }
        }
    }

    /// Only the bare composer works without mouse reporting; every other
    /// surface has scrollable or clickable content.
    pub fn needs_mouse_capture(&self) -> bool {
        match self.layer.as_ref() {
            Some(Layer::Elicitation(modal)) => modal.needs_mouse_capture(),
            Some(Layer::Settings(overlay)) => overlay.needs_mouse_capture(),
            Some(_) => true,
            None => self.composer.has_open_overlay(),
        }
    }

    fn open_git_diff(&mut self) {
        let (screen, task) = GitDiffScreen::new(self.agent.working_dir.clone());
        self.open_layer(Layer::GitDiff(Box::new(screen)));
        self.spawn(task);
    }
}
