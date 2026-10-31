use super::{App, ExitState, Overlay, Route};
use crate::screens::git_diff::GitDiffScreen;
use crate::settings::KeyAction;
use crate::settings::overlay::ProviderLoginStatus;
use crate::surfaces::composer::ComposerOutcome;
use crate::surfaces::dropped_files::parse_dropped_file_paths;
use crate::surfaces::input::{UiOutput, one};
use crate::surfaces::picker::{Builtin, SlashCommand};
use crate::view::input::{MouseAction, UiEvent};
use acp_utils::config_option_id::THEME_CONFIG_ID;
use client_core::composer::attachment::classify_paths;
use client_core::session_config_view::LocalConfigView;
use client_core::session_core::SessionCore;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::time::{Duration, Instant};

pub(super) const CTRL_C_CONFIRM_WINDOW: Duration = Duration::from_secs(1);

impl App {
    pub(super) fn on_terminal_event(&mut self, event: Event) {
        let event = match event {
            Event::Key(key) => UiEvent::Key(key),
            Event::Paste(text) => UiEvent::Paste(text),
            Event::Mouse(mouse) => {
                let Some(action) = MouseAction::from_event(mouse.kind) else { return };
                UiEvent::Mouse(action, (mouse.column, mouse.row), mouse.modifiers)
            }
            Event::FocusLost => {
                self.swallow_next_click = true;
                return;
            }
            Event::Resize(width, _) => {
                self.composer.on_resize(width);
                return;
            }
            Event::FocusGained => return,
        };
        if self.is_refocusing_click(&event) {
            return;
        }
        self.on_ui_event(event);
    }

    fn is_refocusing_click(&mut self, event: &UiEvent) -> bool {
        match event {
            UiEvent::Mouse(MouseAction::Click, ..) => std::mem::take(&mut self.swallow_next_click),
            UiEvent::Key(_) | UiEvent::Paste(_) => {
                self.swallow_next_click = false;
                false
            }
            UiEvent::Mouse(..) => false,
        }
    }

    fn on_ui_event(&mut self, event: UiEvent) {
        let Some(event) = normalize_ui_event(event) else {
            return;
        };
        // Exit handling must precede route and overlay dispatch so no modal can swallow it.
        if let UiEvent::Key(key) = &event
            && self.ui.keybindings.matches(KeyAction::Exit, *key)
        {
            self.arm_or_confirm_exit();
            return;
        }

        if let Some(overlay) = self.overlay.as_mut() {
            let outputs = match overlay {
                Overlay::Settings(overlay) => overlay.on_ui_event(&event, self.core.session()),
                Overlay::Sessions(picker) => picker.on_ui_event(&event),
                Overlay::Workspaces(picker) => picker.on_ui_event(&event),
                Overlay::Elicitation(modal) => one(modal.on_ui_event(&event)),
            };
            self.dispatch_outputs(outputs);
            return;
        }
        if let UiEvent::Key(key) = &event
            && self.ui.keybindings.matches(KeyAction::ToggleGitDiff, *key)
            && matches!(&self.route, Route::GitReview(screen) if screen.is_browsing())
        {
            self.close_active();
            return;
        }
        let outputs = match &mut self.route {
            Route::GitReview(screen) => screen.on_ui_event(&event),
            Route::ArtifactReview(screen) => screen.on_ui_event(&event),
            Route::Conversation => {
                return match &event {
                    UiEvent::Key(key) => self.dispatch_key(*key),
                    UiEvent::Paste(text) => self.on_composer_paste(text),
                    UiEvent::Mouse(..) => self.composer.on_overlay_mouse(&event, self.core.prompt_search()),
                };
            }
        };
        self.dispatch_outputs(outputs);
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
        if let Some(outcome) = self.composer.on_prompt_search_key(key, self.core.prompt_search()) {
            self.apply_composer_outcome(outcome);
            return;
        }

        self.on_composer_key(key);
    }

    /// Acts on the little the composer's overlays cannot do for themselves.
    fn apply_composer_outcome(&mut self, outcome: ComposerOutcome) {
        match outcome {
            ComposerOutcome::Handled => {}
            ComposerOutcome::AcceptedCommand(command) => self.run_accepted_command(command),
            ComposerOutcome::Search(query) => self.update_core(|core| core.search_prompts(&query)),
        }
    }

    fn on_composer_key(&mut self, key: KeyEvent) {
        if self.ui.keybindings.matches(KeyAction::OpenPromptSearch, key)
            && self.core.session().capabilities().prompt_search
            && !self.composer.has_completion()
        {
            self.composer.open_prompt_search();
            self.update_core(|core| core.search_prompts(""));
            return;
        }

        if self.ui.keybindings.matches(KeyAction::ToggleGitDiff, key) {
            let screen = GitDiffScreen::new();
            self.open_route(Route::GitReview(Box::new(screen)));
            self.update_core(SessionCore::open_git_review);
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

        if self.ui.keybindings.matches(KeyAction::CycleReasoning, key) {
            self.apply_config_cycle(|view| view.next_reasoning_effort());
            return;
        }

        if self.ui.keybindings.matches(KeyAction::CycleMode, key) {
            self.apply_config_cycle(|view| view.next_mode());
            return;
        }

        if self.ui.keybindings.matches(KeyAction::Submit, key) {
            self.submit();
            return;
        }

        if self.ui.keybindings.matches(KeyAction::Cancel, key) {
            self.update_core(SessionCore::cancel);
            return;
        }

        if let KeyCode::Char(character) = key.code {
            let opens_command_picker =
                self.ui.keybindings.matches(KeyAction::OpenCommandPicker, key) && self.composer.text().is_empty();
            if opens_command_picker || self.ui.keybindings.matches(KeyAction::OpenFilePicker, key) {
                self.composer.insert_char(character);
                if opens_command_picker {
                    let builtins =
                        Builtin::offered(self.core.session().capabilities()).into_iter().map(SlashCommand::Builtin);
                    let advertised = self.core.available_commands().iter().cloned().map(SlashCommand::Agent);
                    self.composer.open_command_picker(builtins.chain(advertised).collect());
                } else {
                    self.composer.open_file_picker();
                    self.update_core(SessionCore::ensure_git_review);
                    self.composer.on_files_listed(self.core.files());
                }
                return;
            }
        }

        match key.code {
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

    fn apply_config_cycle(&mut self, next: impl FnOnce(LocalConfigView<'_>) -> Option<(&str, &str)>) {
        let view = LocalConfigView::new(self.core.session().config_options());
        if let Some((id, value)) = next(view).map(|(id, value)| (id.to_owned(), value.to_owned())) {
            self.update_core(|core| core.set_config_option(&id, &value));
        }
    }

    /// Builtin commands run as soon as they are accepted; commands taking input
    /// leave the composer ready for it.
    fn run_accepted_command(&mut self, command: SlashCommand) {
        match command {
            SlashCommand::Builtin(builtin) => self.run_builtin(builtin),
            SlashCommand::Agent(command) if command.has_input => self.composer.insert_char(' '),
            SlashCommand::Agent(_) => self.submit(),
        }
    }

    fn submit(&mut self) {
        if self.composer.is_empty() || !self.core.foreground().is_idle() {
            return;
        }
        let submission = self.composer.take_submission();
        self.update_core(|core| core.submit(submission));
    }

    fn on_composer_paste(&mut self, text: &str) {
        if let Some(outcome) = self.composer.on_prompt_search_paste(text, self.core.prompt_search()) {
            self.apply_composer_outcome(outcome);
            return;
        }
        let root = self.core.session().working_dir();
        let added =
            parse_dropped_file_paths(text).is_some_and(|paths| self.composer.attach_files(classify_paths(paths, root)));
        if !added {
            self.composer.insert_paste(text);
        }
        self.composer.refresh_overlay_query();
    }

    pub(super) fn dispatch_outputs(&mut self, outputs: Vec<UiOutput>) {
        for output in outputs {
            self.handle_output(output);
        }
    }

    fn handle_output(&mut self, output: UiOutput) {
        match output {
            UiOutput::Close => self.close_active(),
            UiOutput::ConfigChange(change) => {
                self.apply_settings_change(&change);
                if change.config_id == THEME_CONFIG_ID {
                    self.apply_theme_change(&change.new_value);
                } else {
                    self.update_core(|core| core.set_config_option(&change.config_id, &change.new_value));
                }
            }
            UiOutput::AuthenticateServer(server_name) => {
                self.update_core(|core| core.authenticate_mcp_server(server_name));
            }
            UiOutput::AuthenticateProvider(method_id) => {
                if let Some(Overlay::Settings(overlay)) = self.overlay.as_mut() {
                    overlay.set_provider_status(&method_id, ProviderLoginStatus::Authenticating);
                }
                self.update_core(|core| core.authenticate(method_id));
            }
            UiOutput::ResumeSession { session_id, cwd } => {
                self.update_core(|core| core.resume_session(&session_id, &cwd));
            }
            UiOutput::PreviewSession(session_id) => self.update_core(|core| core.preview_session(session_id)),
            UiOutput::MoveWorkspace(target) => {
                self.close_overlay();
                self.update_core(|core| core.move_workspace(target));
            }
            UiOutput::AnswerElicitation { id, answer } => self.answer_elicitation(id, answer),
            UiOutput::ArtifactReviewed(summary) => {
                self.update_core(|core| core.notify(&summary));
                self.close_active();
            }
            UiOutput::SubmitReview(prompt) => self.update_core(|core| core.submit_review(&prompt)),
            UiOutput::ReviewEvent(event) => self.update_core(|core| core.send_review_event(event)),
            UiOutput::Host(command) => self.queue(command),
        }
    }

    fn run_builtin(&mut self, builtin: Builtin) {
        self.composer.clear();
        match builtin {
            Builtin::Clear => self.update_core(SessionCore::new_session),
            Builtin::Resume => self.update_core(SessionCore::list_sessions),
            Builtin::Settings => self.open_settings(),
            Builtin::Move => self.update_core(SessionCore::begin_workspace_move),
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
            KeyEventKind::Release if matches!(key.code, KeyCode::Modifier(_)) => Some(UiEvent::Key(key)),
            KeyEventKind::Release => None,
        },
        event => Some(event),
    }
}
