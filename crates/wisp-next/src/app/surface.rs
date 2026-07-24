use super::config::{cycle_quick_option, cycle_reasoning_option, update_config_option_value};
use super::{
    ActiveSurface, App, Duration, Event, ExitState, GitDiffEvent, Instant, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, ModalOutcome, OverlayLayer, PromptSearchMessage, Rect, ScreenEffect, ScreenEvent,
    parse_dropped_file_paths,
};

pub(super) const CTRL_C_CONFIRM_WINDOW: Duration = Duration::from_secs(1);

impl App {
    #[allow(clippy::too_many_lines)]
    pub fn on_key(&mut self, key: KeyEvent) {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }

        'event: {
            if self.keybindings.exit.matches(key) {
                if self.exit_state.is_confirming() {
                    self.exit_state = ExitState::Exiting;
                } else {
                    self.composer.clear();
                    self.exit_state = ExitState::Confirming(Instant::now());
                }
                break 'event;
            }

            match self.active_surface() {
                ActiveSurface::Screen => {
                    if let Some(effect) = self.screen_router.on_key(key) {
                        self.pending_screen_effects.push_back(effect);
                    }
                    break 'event;
                }
                ActiveSurface::Settings => {
                    let messages = if let OverlayLayer::Settings(overlay) = &mut self.overlay {
                        overlay.on_key(key)
                    } else {
                        Vec::new()
                    };
                    for message in messages {
                        self.handle_settings_message(message);
                    }
                    break 'event;
                }
                ActiveSurface::SessionPicker => {
                    let messages = if let OverlayLayer::SessionPicker(picker) = &mut self.overlay {
                        picker.on_key(key).unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    for message in messages {
                        self.handle_session_picker_message(message);
                    }
                    break 'event;
                }
                ActiveSurface::WorkspacePicker => {
                    let messages = if let OverlayLayer::WorkspacePicker(picker) = &mut self.overlay {
                        picker.on_key(key).unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    for message in messages {
                        self.handle_workspace_picker_message(message);
                    }
                    break 'event;
                }
                ActiveSurface::Modal => {
                    if let OverlayLayer::Elicitation(modal) = &mut self.overlay
                        && matches!(modal.on_key(key), ModalOutcome::Close)
                    {
                        self.overlay = OverlayLayer::None;
                    }
                    break 'event;
                }
                ActiveSurface::PromptSearch => {
                    if let Some(msg) = self.composer.prompt_search_on_key(key) {
                        match msg {
                            PromptSearchMessage::QueryChanged(query) if !query.trim().is_empty() => {
                                self.send_prompt_search_query(query);
                            }
                            PromptSearchMessage::QueryChanged(_) => self.composer.restore_prompt_search_draft(),
                            _ => {}
                        }
                    }
                    break 'event;
                }
                ActiveSurface::Overlay | ActiveSurface::Composer => {}
            }

            if self.keybindings.open_prompt_search.matches(key)
                && self.capabilities.prompt_search
                && !self.composer.has_overlay()
            {
                self.composer.open_prompt_search();
                break 'event;
            }

            if self.keybindings.toggle_git_diff.matches(key) {
                let effect = self.screen_router.open_git_diff(&self.working_dir);
                self.pending_screen_effects.push_back(effect);
                break 'event;
            }

            if key.code == KeyCode::Enter && key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT)
                || key.code == KeyCode::Char('j') && key.modifiers.contains(KeyModifiers::CONTROL)
            {
                self.composer.insert_newline();
                break 'event;
            }

            if self.active_surface() == ActiveSurface::Overlay {
                self.on_overlay_key(key);
                break 'event;
            }

            if self.keybindings.cycle_reasoning.matches(key) {
                if let Some((id, val)) = cycle_reasoning_option(&self.config_options)
                    && self.prompt_handle.set_config_option(&self.session_id, &id, &val).is_ok()
                {
                    update_config_option_value(&mut self.config_options, &id, &val);
                }
                break 'event;
            }

            if self.keybindings.cycle_mode.matches(key) {
                if let Some((id, val)) = cycle_quick_option(&self.config_options)
                    && self.prompt_handle.set_config_option(&self.session_id, &id, &val).is_ok()
                {
                    update_config_option_value(&mut self.config_options, &id, &val);
                }
                break 'event;
            }

            if self.keybindings.submit.matches(key) {
                self.submit();
                break 'event;
            }

            if self.keybindings.cancel.matches(key) {
                if self.prompt_in_flight {
                    let _ = self.prompt_handle.cancel(&self.session_id);
                }
                break 'event;
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
                KeyCode::Backspace => self.composer.backspace(),
                KeyCode::Delete => self.composer.delete(),
                KeyCode::Left => self.composer.move_left(),
                KeyCode::Right => self.composer.move_right(),
                KeyCode::Up if !self.composer.move_up() => {
                    self.composer.recall_previous();
                }
                KeyCode::Down if !self.composer.move_down() => {
                    self.composer.recall_next();
                }
                KeyCode::Home => self.composer.move_line_start(),
                KeyCode::End => self.composer.move_line_end(),
                KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => self.composer.move_line_start(),
                KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => self.composer.move_line_end(),
                _ => {}
            }
        }
        self.refresh_progress();
    }

    pub fn on_paste(&mut self, text: &str) {
        if self.active_surface() == ActiveSurface::PromptSearch {
            if let Some(msg) = self.composer.prompt_search_on_paste(text) {
                match msg {
                    PromptSearchMessage::QueryChanged(query) if !query.trim().is_empty() => {
                        self.send_prompt_search_query(query);
                    }
                    PromptSearchMessage::QueryChanged(_) => {
                        self.composer.restore_prompt_search_draft();
                    }
                    _ => {}
                }
            }
            return;
        }
        let added = parse_dropped_file_paths(text).is_some_and(|paths| self.composer.add_dropped_media(paths));
        if !added {
            self.composer.insert_paste(text);
        }
        self.composer.refresh_overlay_query();
    }
    pub fn render_modal(
        &mut self,
        frame: &mut ratatui::Frame,
        theme: &crate::theme::Theme,
        highlighter: &mut crate::syntax::SyntaxHighlighter,
        theme_generation: u64,
    ) {
        match self.active_surface() {
            ActiveSurface::PromptSearch | ActiveSurface::Overlay => {}
            ActiveSurface::Composer => self.surface_rect = None,
            ActiveSurface::Screen => {
                self.surface_rect = Some(frame.area());
                self.screen_router.render(frame, theme, highlighter, theme_generation);
            }
            ActiveSurface::Settings => {
                let area = frame.area();
                self.surface_rect = Some(area);
                if let OverlayLayer::Settings(overlay) = &mut self.overlay {
                    overlay.render(area, frame.buffer_mut(), theme);
                }
            }
            ActiveSurface::SessionPicker => {
                let area = frame.area();
                self.surface_rect = Some(area);
                if let OverlayLayer::SessionPicker(picker) = &mut self.overlay {
                    picker.render(area, frame.buffer_mut(), theme);
                }
            }
            ActiveSurface::WorkspacePicker => {
                let area = frame.area();
                self.surface_rect = Some(area);
                if let OverlayLayer::WorkspacePicker(picker) = &mut self.overlay {
                    picker.render(area, frame.buffer_mut(), theme);
                }
            }
            ActiveSurface::Modal => {
                self.surface_rect = Some(frame.area());
                if let OverlayLayer::Elicitation(modal) = &self.overlay {
                    modal.render(frame, theme);
                }
            }
        }
    }

    pub fn on_screen_event(&mut self, event: ScreenEvent) {
        if let ScreenEvent::GitDiff(GitDiffEvent::SubmitReview { request_id: _, prompt }) = &event {
            if self.prompt_in_flight {
                return;
            }
            let prompt = prompt.clone();
            self.prompt_in_flight = true;
            self.transcript.push_user_message(&format!("[wisp-next] Submitted review of working tree diff.\n{prompt}"));
            if let Err(e) = self.prompt_handle.prompt(&self.session_id, &prompt, None) {
                tracing::error!("failed to send review prompt: {e}");
                self.prompt_in_flight = false;
                self.transcript.push_user_message(&format!("[wisp-next] Failed to send review: {e}"));
            } else {
                self.screen_router.close();
            }
            self.screen_router.on_event(event);
            return;
        }
        if let Some(effect) = self.screen_router.on_event(event) {
            self.pending_screen_effects.push_back(effect);
        }
    }

    pub fn take_screen_effect(&mut self) -> Option<ScreenEffect> {
        self.pending_screen_effects.pop_front()
    }

    pub fn take_bell(&mut self) -> bool {
        self.pending_bell.take().is_some()
    }

    pub fn needs_mouse_capture(&self) -> bool {
        self.has_mouse_capturing_surface()
    }

    fn has_mouse_capturing_surface(&self) -> bool {
        match self.active_surface() {
            ActiveSurface::Composer => false,
            ActiveSurface::Modal => {
                if let OverlayLayer::Elicitation(modal) = &self.overlay {
                    modal.needs_mouse_capture()
                } else {
                    false
                }
            }
            ActiveSurface::Screen
            | ActiveSurface::Settings
            | ActiveSurface::SessionPicker
            | ActiveSurface::WorkspacePicker
            | ActiveSurface::PromptSearch
            | ActiveSurface::Overlay => true,
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

    pub fn clear_surface_rect(&mut self) {
        self.surface_rect = None;
    }

    pub fn on_terminal_event(&mut self, event: Event) {
        match event {
            Event::Key(key) => self.on_key(key),
            Event::Paste(text) => self.on_paste(&text),
            Event::Resize(width, height) => self.on_resize(width, height),
            Event::Mouse(mouse) => self.on_mouse(mouse),
            _ => {}
        }
    }
    fn on_overlay_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.composer.close_overlay(),
            KeyCode::Up => self.composer.overlay_move_up(),
            KeyCode::Down => self.composer.overlay_move_down(),
            KeyCode::Enter | KeyCode::Tab => {
                if let Some(command) = self.composer.accept_command() {
                    if command.builtin {
                        self.dispatch_builtin_command(&command);
                    } else if command.has_input {
                        self.composer.insert_char(' ');
                    } else {
                        self.submit();
                    }
                } else {
                    self.composer.accept_file();
                }
            }
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
    fn on_resize(&mut self, width: u16, height: u16) {
        self.last_terminal_size = (width, height);
        self.surface_rect = None;
    }

    fn on_mouse(&mut self, event: crossterm::event::MouseEvent) {
        use crossterm::event::MouseEventKind;
        let Some(rect) = self.surface_rect else {
            return;
        };
        let col = event.column;
        let row = event.row;
        if col < rect.x || col >= rect.right() || row < rect.y || row >= rect.bottom() {
            return;
        }
        let local_x = col.saturating_sub(rect.x);
        let local_y = row.saturating_sub(rect.y);
        match event.kind {
            MouseEventKind::ScrollUp => self.surface_scroll_up(local_y, local_x),
            MouseEventKind::ScrollDown => self.surface_scroll_down(local_y, local_x),
            MouseEventKind::Down(_) => self.surface_click(local_y, local_x),
            _ => {}
        }
    }

    fn surface_scroll_up(&mut self, local_y: u16, local_x: u16) {
        match &mut self.overlay {
            OverlayLayer::Settings(o) => o.on_mouse_scroll_up(local_y),
            OverlayLayer::SessionPicker(p) => p.scroll_up(),
            OverlayLayer::WorkspacePicker(p) => p.scroll_up(),
            OverlayLayer::Elicitation(m) => m.on_mouse_scroll_up(local_y),
            OverlayLayer::None => {
                if self.screen_router.is_active() {
                    self.screen_router.on_mouse_scroll_up(local_y, local_x);
                } else if self.composer.has_prompt_search() {
                    self.composer.prompt_search_move_up();
                } else if self.composer.has_overlay() {
                    self.composer.overlay_move_up();
                }
            }
        }
    }

    fn surface_scroll_down(&mut self, local_y: u16, local_x: u16) {
        match &mut self.overlay {
            OverlayLayer::Settings(o) => o.on_mouse_scroll_down(local_y),
            OverlayLayer::SessionPicker(p) => p.scroll_down(),
            OverlayLayer::WorkspacePicker(p) => p.scroll_down(),
            OverlayLayer::Elicitation(m) => m.on_mouse_scroll_down(local_y),
            OverlayLayer::None => {
                if self.screen_router.is_active() {
                    self.screen_router.on_mouse_scroll_down(local_y, local_x);
                } else if self.composer.has_prompt_search() {
                    self.composer.prompt_search_move_down();
                } else if self.composer.has_overlay() {
                    self.composer.overlay_move_down();
                }
            }
        }
    }

    fn surface_click(&mut self, local_y: u16, local_x: u16) {
        let settings_messages = match &mut self.overlay {
            OverlayLayer::Settings(o) => Some(o.on_mouse_click(local_y, self.surface_rect.unwrap_or_default())),
            OverlayLayer::SessionPicker(p) => {
                p.select_row(local_y.saturating_sub(1) as usize);
                None
            }
            OverlayLayer::WorkspacePicker(p) => {
                p.select_row(local_y.saturating_sub(1) as usize);
                None
            }
            OverlayLayer::Elicitation(m) => {
                m.on_mouse_click(local_y);
                None
            }
            OverlayLayer::None => {
                if self.screen_router.is_active() {
                    self.screen_router.on_mouse_click(local_y, local_x);
                } else if self.composer.has_prompt_search() {
                    self.composer.prompt_search_select_row(local_y as usize);
                } else if self.composer.has_overlay() {
                    self.composer.overlay_select_row(local_y as usize);
                }
                None
            }
        };
        if let Some(messages) = settings_messages {
            for message in messages {
                self.handle_settings_message(message);
            }
        }
    }
}
