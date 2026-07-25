use super::config::{cycle_quick_option, cycle_reasoning_option, update_config_option_value};
use super::{
    ActiveSurface, App, Direction, Duration, Event, ExitState, Instant, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    OverlayLayer, PromptSearchMessage, Rect, ScreenEffect, ScreenEvent, parse_dropped_file_paths,
};
use crate::overlay::Overlay;
use crate::screens::git_diff::GitDiffScreen;
use crate::screens::{MouseAction, RenderContext, Screen, ScreenOutcome};
use crossterm::event::{MouseEvent, MouseEventKind};
use ratatui::Frame;

pub(super) const CTRL_C_CONFIRM_WINDOW: Duration = Duration::from_secs(1);

impl App {
    pub fn on_terminal_event(&mut self, event: Event) {
        match event {
            Event::Key(key) => self.on_key(key),
            Event::Paste(text) => self.on_paste(&text),
            Event::Resize(width, height) => {
                self.last_terminal_size = (width, height);
                self.surface_rect = None;
            }
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

        if let Some(screen) = self.screen.as_mut() {
            match screen.on_key(key) {
                ScreenOutcome::None => {}
                ScreenOutcome::Close => self.close_screen(),
                ScreenOutcome::Effect(effect) => self.pending_screen_effects.push_back(effect),
            }
            return;
        }

        if let Some(overlay) = self.overlay.active() {
            let messages = overlay.on_key(key);
            self.handle_overlay_messages(messages);
            return;
        }

        if self.composer.has_prompt_search() {
            self.on_prompt_search_key(key);
            return;
        }

        self.on_composer_key(key);
    }

    fn on_prompt_search_key(&mut self, key: KeyEvent) {
        let message = self.composer.prompt_search_on_key(key);
        self.apply_prompt_search_message(message);
    }

    /// A new query goes to the agent; an emptied one restores the draft the
    /// search replaced.
    fn apply_prompt_search_message(&mut self, message: Option<PromptSearchMessage>) {
        match message {
            Some(PromptSearchMessage::QueryChanged(query)) if !query.trim().is_empty() => {
                self.send_prompt_search_query(query);
            }
            Some(PromptSearchMessage::QueryChanged(_)) => self.composer.restore_prompt_search_draft(),
            _ => {}
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
            if self.prompt_in_flight {
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
                    self.composer.open_file_picker(&self.working_dir);
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
            match direction {
                Direction::Backward => overlay.move_up(),
                Direction::Forward => overlay.move_down(),
            }
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
            let message = self.composer.prompt_search_on_paste(text);
            self.apply_prompt_search_message(message);
            return;
        }
        let added = parse_dropped_file_paths(text).is_some_and(|paths| self.composer.add_dropped_media(paths));
        if !added {
            self.composer.insert_paste(text);
        }
        self.composer.refresh_overlay_query();
    }

    /// Draws the full-screen view or overlay on top of the transcript, and
    /// records the area mouse events are measured against.
    pub fn render_active_surface(&mut self, frame: &mut Frame, cx: &mut RenderContext<'_>) {
        let area = frame.area();
        if let Some(screen) = self.screen.as_mut() {
            self.surface_rect = Some(area);
            if let Some(cursor) = screen.render(area, frame.buffer_mut(), cx) {
                frame.set_cursor_position(cursor);
            }
            return;
        }
        let theme = cx.theme;
        if let Some(overlay) = self.overlay.active() {
            overlay.render(area, frame.buffer_mut(), theme);
            self.surface_rect = Some(area);
            return;
        }
        // The composer's own surfaces set `surface_rect` while they render.
        if !self.composer.has_prompt_search() && !self.composer.has_overlay() {
            self.surface_rect = None;
        }
    }

    pub fn on_screen_event(&mut self, event: ScreenEvent) {
        if let ScreenEvent::SubmitReview { prompt, .. } = &event {
            self.submit_review(prompt);
        }
        if let Some(effect) = self.screen.as_mut().and_then(|screen| screen.on_event(event)) {
            self.pending_screen_effects.push_back(effect);
        }
    }

    pub fn take_screen_effect(&mut self) -> Option<ScreenEffect> {
        self.pending_screen_effects.pop_front()
    }

    pub fn take_bell(&mut self) -> bool {
        self.pending_bell.take().is_some()
    }

    /// Only the composer works without mouse reporting; every other surface has
    /// scrollable or clickable content.
    pub fn needs_mouse_capture(&self) -> bool {
        match self.active_surface() {
            ActiveSurface::Composer => false,
            ActiveSurface::Modal => {
                matches!(&self.overlay, OverlayLayer::Elicitation(modal) if modal.needs_mouse_capture())
            }
            _ => true,
        }
    }

    pub fn terminal_size(&self) -> (u16, u16) {
        self.last_terminal_size
    }

    pub fn surface_rect(&self) -> Option<Rect> {
        self.surface_rect
    }

    pub fn set_surface_rect(&mut self, rect: Rect) {
        self.surface_rect = Some(rect);
    }

    pub(super) fn open_screen(&mut self, screen: Box<dyn Screen>) {
        self.close_screen();
        self.screen = Some(screen);
    }

    pub(super) fn close_screen(&mut self) {
        if let Some(mut screen) = self.screen.take() {
            screen.cancel();
        }
    }

    fn open_git_diff(&mut self) {
        let (screen, effect) = GitDiffScreen::new(self.working_dir.clone());
        self.open_screen(Box::new(screen));
        self.pending_screen_effects.push_back(effect);
    }

    fn on_mouse(&mut self, event: MouseEvent) {
        let Some(rect) = self.surface_rect else {
            return;
        };
        if event.column < rect.x || event.column >= rect.right() || event.row < rect.y || event.row >= rect.bottom() {
            return;
        }
        let action = match event.kind {
            MouseEventKind::ScrollUp => MouseAction::ScrollUp,
            MouseEventKind::ScrollDown => MouseAction::ScrollDown,
            MouseEventKind::Down(_) => MouseAction::Click,
            _ => return,
        };
        let row = event.row - rect.y;
        let column = event.column - rect.x;

        if let Some(screen) = self.screen.as_mut() {
            screen.on_mouse(action, row, column);
            return;
        }
        if let Some(overlay) = self.overlay.active() {
            let messages = match action {
                MouseAction::ScrollUp => overlay.scroll(Direction::Backward),
                MouseAction::ScrollDown => overlay.scroll(Direction::Forward),
                MouseAction::Click => overlay.click(row, rect),
            };
            self.handle_overlay_messages(messages);
            return;
        }
        self.composer_mouse(action, row);
    }

    fn composer_mouse(&mut self, action: MouseAction, row: u16) {
        if self.composer.has_prompt_search() {
            self.composer.navigate_prompt_search(|picker| match action {
                MouseAction::ScrollUp => picker.move_up(),
                MouseAction::ScrollDown => picker.move_down(),
                // Row 0 is the search header.
                MouseAction::Click => picker.select_row(usize::from(row.saturating_sub(1))),
            });
        } else if let Some(overlay) = self.composer.completion() {
            match action {
                MouseAction::ScrollUp => overlay.move_up(),
                MouseAction::ScrollDown => overlay.move_down(),
                // Row 0 is the rule separating the list from the composer.
                MouseAction::Click => overlay.select_row(usize::from(row.saturating_sub(1))),
            }
        }
    }
}
