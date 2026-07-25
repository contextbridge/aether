use super::config::{cycle_quick_option, cycle_reasoning_option, update_config_option_value};
use super::{App, ExitState, Layer};
use crate::dropped_files::parse_dropped_file_paths;
use crate::effects::{Effect, EffectResult};
use crate::render_context::RenderContext;
use crate::screens::git_diff::GitDiffScreen;
use crate::selection::Direction;
use crate::surface::{MouseAction, Surface};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use std::time::{Duration, Instant};

pub(super) const CTRL_C_CONFIRM_WINDOW: Duration = Duration::from_secs(1);

impl App {
    pub fn on_terminal_event(&mut self, event: Event) {
        match event {
            Event::Key(key) => self.on_key(key),
            Event::Paste(text) => self.on_paste(&text),
            Event::Resize(width, height) => self.last_terminal_size = (width, height),
            Event::Mouse(mouse) => self.on_mouse(mouse),
            _ => {}
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }
        self.dispatch_key(key);
        self.refresh_progress();
    }

    /// Routes a keystroke to whichever layer owns input, innermost first.
    fn dispatch_key(&mut self, key: KeyEvent) {
        if self.keybindings.exit.matches(key) {
            if self.exit_state.is_confirming() {
                self.exit_state = ExitState::Exiting;
            } else {
                self.composer.clear();
                self.exit_state = ExitState::Confirming(Instant::now());
            }
            return;
        }

        if let Some(layer) = self.layer.as_mut() {
            let messages = layer.surface().on_key(key);
            self.handle_surface_messages(messages);
            return;
        }

        if self.composer.has_prompt_search() {
            let query = self.composer.prompt_search_on_key(key);
            self.apply_prompt_search_query(query);
            return;
        }

        self.on_composer_key(key);
    }

    /// A new query goes to the agent; an emptied one restores the draft the
    /// search replaced.
    fn apply_prompt_search_query(&mut self, query: Option<String>) {
        match query {
            Some(query) if !query.trim().is_empty() => self.send_prompt_search_query(query),
            Some(_) => self.composer.restore_prompt_search_draft(),
            None => {}
        }
    }

    fn on_composer_key(&mut self, key: KeyEvent) {
        if self.keybindings.open_prompt_search.matches(key)
            && self.capabilities.prompt_search
            && !self.composer.has_overlay()
        {
            self.composer.open_prompt_search();
            return;
        }

        if self.keybindings.toggle_git_diff.matches(key) {
            self.open_git_diff();
            return;
        }

        if key.code == KeyCode::Enter && key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT)
            || key.code == KeyCode::Char('j') && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.composer.insert_newline();
            return;
        }

        if self.composer.has_overlay() {
            self.on_completion_key(key);
            return;
        }

        if self.keybindings.cycle_reasoning.matches(key) {
            self.apply_config_cycle(cycle_reasoning_option(&self.config_options));
            return;
        }

        if self.keybindings.cycle_mode.matches(key) {
            self.apply_config_cycle(cycle_quick_option(&self.config_options));
            return;
        }

        if self.keybindings.submit.matches(key) {
            self.submit();
            return;
        }

        if self.keybindings.cancel.matches(key) {
            if self.turn.prompt_in_flight {
                let _ = self.prompt_handle.cancel(&self.session_id);
            }
            return;
        }

        match key.code {
            KeyCode::Char(character) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.composer.insert_char(character);
                if self.keybindings.open_command_picker.matches(key) && self.composer.text() == "/" {
                    self.composer.open_command_picker(self.available_commands.clone());
                } else if self.keybindings.open_file_picker.matches(key) {
                    let effect = self.composer.open_file_picker(&self.working_dir);
                    self.pending_effects.push_back(effect);
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
            && self.prompt_handle.set_config_option(&self.session_id, &id, &value).is_ok()
        {
            update_config_option_value(&mut self.config_options, &id, &value);
        }
    }

    fn on_completion_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.composer.close_overlay(),
            KeyCode::Up => self.move_completion(Direction::Backward),
            KeyCode::Down => self.move_completion(Direction::Forward),
            KeyCode::Enter | KeyCode::Tab => self.accept_completion(),
            KeyCode::Backspace if self.composer.active_token_is_empty() => {
                self.composer.backspace();
                self.composer.close_overlay();
            }
            KeyCode::Backspace => {
                self.composer.backspace();
                self.composer.refresh_overlay_query();
            }
            KeyCode::Char(character) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.composer.insert_char(character);
                if character.is_whitespace() {
                    self.composer.close_overlay();
                } else {
                    self.composer.refresh_overlay_query();
                }
            }
            _ => {}
        }
    }

    fn move_completion(&mut self, direction: Direction) {
        if let Some(overlay) = self.composer.completion() {
            overlay.step(direction);
        }
    }

    /// Accepts the focused completion. Builtin commands run immediately;
    /// commands taking input leave the composer ready for it.
    fn accept_completion(&mut self) {
        let Some(command) = self.composer.accept_command() else {
            self.composer.accept_file();
            return;
        };
        if command.builtin {
            self.dispatch_builtin_command(&command);
        } else if command.has_input {
            self.composer.insert_char(' ');
        } else {
            self.submit();
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        if self.composer.has_prompt_search() {
            let query = self.composer.prompt_search_on_paste(text);
            self.apply_prompt_search_query(query);
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
    pub fn render_layer(&mut self, area: Rect, buf: &mut Buffer, cx: &mut RenderContext<'_>) -> Option<Position> {
        self.layer.as_mut()?.surface().render(area, buf, cx)
    }

    /// Routes an effect result: file-index results belong to the composer, and
    /// everything else to whichever surface asked for the work.
    pub fn on_effect_result(&mut self, result: EffectResult) {
        let event = match result {
            EffectResult::FilesIndexed { request_id, files } => {
                self.composer.on_files_indexed(request_id, files);
                return;
            }
            EffectResult::Surface(event) => event,
        };
        let Some(layer) = self.layer.as_mut() else {
            return;
        };
        let messages = layer.surface().on_event(event);
        self.handle_surface_messages(messages);
    }

    pub fn take_effect(&mut self) -> Option<Effect> {
        self.pending_effects.pop_front()
    }

    pub fn take_bell(&mut self) -> bool {
        self.pending_bell.take().is_some()
    }

    /// Only the bare composer works without mouse reporting; every other
    /// surface has scrollable or clickable content.
    pub fn needs_mouse_capture(&self) -> bool {
        match self.layer.as_ref() {
            Some(Layer::Elicitation(modal)) => modal.needs_mouse_capture(),
            Some(_) => true,
            None => self.composer.has_open_overlay(),
        }
    }

    pub fn terminal_size(&self) -> (u16, u16) {
        self.last_terminal_size
    }

    fn open_git_diff(&mut self) {
        let (screen, effect) = GitDiffScreen::new(self.working_dir.clone());
        self.open_layer(Layer::GitDiff(Box::new(screen)));
        self.pending_effects.push_back(effect.into());
    }

    /// Routes a mouse event to whatever owns input.
    ///
    /// Rows and columns stay in terminal coordinates the whole way down: each
    /// list records the area it drew its rows into, so none of them has to
    /// re-derive how many borders and headers sit above the first row, and a
    /// click that lands on nothing is simply ignored.
    fn on_mouse(&mut self, event: MouseEvent) {
        let Some(action) = MouseAction::from_event(event.kind) else {
            return;
        };
        if let Some(layer) = self.layer.as_mut() {
            let messages = layer.surface().on_mouse(action, event.row, event.column);
            self.handle_surface_messages(messages);
            return;
        }
        self.composer.on_overlay_mouse(action, event.row);
    }
}
