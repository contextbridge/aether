use crate::components::app::attachments::{AttachmentKind, classify_attachment};
use crate::components::command_picker::{CommandEntry, CommandPicker, CommandPickerMessage};
use crate::components::dropped_files::parse_dropped_file_paths;
use crate::components::file_picker::{FilePicker, FilePickerMessage};
use crate::components::input_prompt::{InputPrompt, prompt_content_width};
use crate::components::text_input::{SelectedFileMention, TextInput, TextInputMessage};
use crate::keybindings::Keybindings;
use std::collections::HashSet;
use std::path::PathBuf;
use tui::{Component, Cursor, Event, Frame, KeyCode, KeyModifiers, Line, PickerMessage, ViewContext};

use super::app::PromptAttachment;

#[derive(Debug)]
pub enum PromptComposerMessage {
    SubmitRequested { user_input: String, attachments: Vec<PromptAttachment> },
    OpenSettings,
    OpenSessionPicker,
    NewSession,
}

pub struct PromptComposer {
    text_input: TextInput,
    available_commands: Vec<CommandEntry>,
    file_picker: Option<FilePicker>,
    command_picker: Option<CommandPicker>,
    pending_media: Vec<PromptAttachment>,
}

impl Default for PromptComposer {
    fn default() -> Self {
        Self::new(Keybindings::default())
    }
}

impl PromptComposer {
    pub fn new(keybindings: Keybindings) -> Self {
        Self {
            text_input: TextInput::new(keybindings),
            available_commands: Vec::new(),
            file_picker: None,
            command_picker: None,
            pending_media: Vec::new(),
        }
    }

    pub fn set_available_commands(&mut self, commands: Vec<CommandEntry>) {
        self.available_commands = commands;
    }

    #[cfg(test)]
    pub fn has_active_picker(&self) -> bool {
        self.file_picker.is_some() || self.command_picker.is_some()
    }

    #[cfg(test)]
    pub(crate) fn set_input(&mut self, input: String) {
        self.text_input.set_input(input);
    }

    #[cfg(test)]
    pub(crate) fn buffer(&self) -> &str {
        self.text_input.buffer()
    }

    #[cfg(test)]
    pub(crate) fn cursor_index(&self) -> usize {
        let picker_query_len = self.file_picker.as_ref().map(|picker| picker.query().len());
        self.text_input.cursor_index(picker_query_len)
    }

    #[cfg(test)]
    pub(crate) fn apply_file_selection(&mut self, path: std::path::PathBuf, display_name: String) {
        self.text_input.apply_file_selection(path, display_name);
    }

    #[cfg(test)]
    pub(crate) fn open_command_picker_with_entries(&mut self, commands: Vec<CommandEntry>) {
        self.command_picker = Some(CommandPicker::new(commands));
    }

    #[cfg(test)]
    pub(crate) fn pending_media(&self) -> &[PromptAttachment] {
        &self.pending_media
    }

    pub fn close_all(&mut self) {
        self.file_picker = None;
        self.command_picker = None;
    }

    pub fn cursor(&self, context: &ViewContext) -> Cursor {
        let picker_query_len = self.file_picker.as_ref().map(|picker| picker.query().len());
        let layout = InputPrompt {
            input: self.text_input.buffer(),
            cursor_index: self.text_input.cursor_index(picker_query_len),
        }
        .layout(context);

        Cursor { row: layout.cursor_row, col: layout.cursor_col as usize, is_visible: true }
    }

    #[cfg(test)]
    pub(crate) fn has_file_picker(&self) -> bool {
        self.file_picker.is_some()
    }

    #[cfg(test)]
    pub(crate) fn has_command_picker(&self) -> bool {
        self.command_picker.is_some()
    }

    fn handle_picker_outcome<T>(&mut self, outcome: Option<Vec<PickerMessage<T>>>) -> (bool, Option<T>) {
        let Some(msg) = outcome.unwrap_or_default().into_iter().next() else {
            return (false, None);
        };
        match msg {
            PickerMessage::Close => (true, None),
            PickerMessage::CloseAndPopChar => {
                self.text_input.delete_char_before_cursor();
                (true, None)
            }
            PickerMessage::CloseWithChar(c) => {
                self.text_input.insert_char_at_cursor(c);
                (true, None)
            }
            PickerMessage::Confirm(value) => (true, Some(value)),
            PickerMessage::CharTyped(c) => {
                self.text_input.insert_char_at_cursor(c);
                (false, None)
            }
            PickerMessage::PopChar => {
                self.text_input.delete_char_before_cursor();
                (false, None)
            }
        }
    }

    fn handle_file_picker_outcome(&mut self, outcome: Option<Vec<FilePickerMessage>>) -> Vec<PromptComposerMessage> {
        let (close, confirmed) = self.handle_picker_outcome(outcome);
        if let Some(file_match) = confirmed {
            self.file_picker = None;
            self.text_input.apply_file_selection(file_match.path, file_match.display_name);
        } else if close {
            self.file_picker = None;
        }
        vec![]
    }

    fn handle_command_picker_outcome(
        &mut self,
        outcome: Option<Vec<CommandPickerMessage>>,
    ) -> Vec<PromptComposerMessage> {
        let (close, confirmed) = self.handle_picker_outcome(outcome);
        if let Some(cmd) = confirmed {
            self.command_picker = None;
            return self.apply_command(&cmd);
        } else if close {
            self.command_picker = None;
        }
        vec![]
    }

    fn handle_text_input_outcome(
        &mut self,
        outcome: Option<Vec<TextInputMessage>>,
    ) -> Option<Vec<PromptComposerMessage>> {
        let msgs = outcome?;
        match msgs.into_iter().next() {
            Some(TextInputMessage::Submit) => Some(self.prepare_submit()),
            Some(TextInputMessage::OpenCommandPicker) => {
                let mut commands = builtin_commands();
                commands.extend(self.available_commands.clone());
                self.command_picker = Some(CommandPicker::new(commands));
                Some(vec![])
            }
            Some(TextInputMessage::OpenFilePicker) => {
                self.file_picker = Some(FilePicker::new());
                Some(vec![])
            }
            None => Some(vec![]),
        }
    }

    fn apply_command(&mut self, cmd: &CommandEntry) -> Vec<PromptComposerMessage> {
        if cmd.builtin && cmd.name == "clear" {
            self.text_input.clear();
            self.close_all();
            vec![PromptComposerMessage::NewSession]
        } else if cmd.builtin && cmd.name == "settings" {
            self.text_input.clear();
            self.close_all();
            vec![PromptComposerMessage::OpenSettings]
        } else if cmd.builtin && cmd.name == "resume" {
            self.text_input.clear();
            self.close_all();
            vec![PromptComposerMessage::OpenSessionPicker]
        } else if cmd.has_input {
            self.text_input.set_input(format!("/{} ", cmd.name));
            vec![]
        } else {
            self.text_input.set_input(format!("/{}", cmd.name));
            self.prepare_submit()
        }
    }

    fn add_dropped_media(&mut self, paths: Vec<PathBuf>) -> bool {
        let mut existing: HashSet<PathBuf> =
            self.pending_media.iter().filter_map(|a| std::fs::canonicalize(&a.path).ok()).collect();

        let before = self.pending_media.len();

        for path in paths {
            let kind = classify_attachment(&path);
            if !matches!(kind, AttachmentKind::Image | AttachmentKind::Audio) {
                continue;
            }
            if let Ok(canon) = std::fs::canonicalize(&path)
                && !existing.insert(canon)
            {
                continue;
            }
            let display_name = path
                .file_name()
                .map_or_else(|| path.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
            self.pending_media.push(PromptAttachment { path, display_name });
        }

        self.pending_media.len() > before
    }

    fn prepare_submit(&mut self) -> Vec<PromptComposerMessage> {
        let has_text = !self.text_input.buffer().trim().is_empty();
        let has_media = !self.pending_media.is_empty();

        if !has_text && !has_media {
            return vec![];
        }

        let user_input = self.text_input.buffer().trim().to_string();
        self.text_input.record_submission(&user_input);
        let mut attachments = collect_submit_attachments(&user_input, self.text_input.take_mentions());
        attachments.extend(std::mem::take(&mut self.pending_media));
        self.text_input.clear();
        self.close_all();

        vec![PromptComposerMessage::SubmitRequested { user_input, attachments }]
    }
}

impl Component for PromptComposer {
    type Message = PromptComposerMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        match event {
            Event::Paste(text) => {
                self.close_all();
                let added = parse_dropped_file_paths(text).is_some_and(|paths| self.add_dropped_media(paths));
                if !added {
                    self.text_input.insert_paste(text);
                }
                Some(vec![])
            }
            Event::Key(key_event) => {
                if key_event.code == KeyCode::Enter && key_event.modifiers.contains(KeyModifiers::SHIFT) {
                    self.close_all();
                    let outcome = self.text_input.on_event(event).await;
                    return self.handle_text_input_outcome(outcome);
                }

                if let Some(ref mut picker) = self.file_picker {
                    let outcome = picker.on_event(event).await;
                    if outcome.is_some() {
                        return Some(self.handle_file_picker_outcome(outcome));
                    }

                    if matches!(
                        key_event.code,
                        KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End | KeyCode::Up | KeyCode::Down
                    ) {
                        return Some(vec![]);
                    }
                }

                if let Some(ref mut picker) = self.command_picker {
                    let outcome = picker.on_event(event).await;
                    return Some(self.handle_command_picker_outcome(outcome));
                }

                // Backspace on empty prompt removes the last dropped media attachment
                if key_event.code == KeyCode::Backspace
                    && self.text_input.buffer().is_empty()
                    && !self.pending_media.is_empty()
                {
                    self.pending_media.pop();
                    return Some(vec![]);
                }

                let outcome = self.text_input.on_event(event).await;
                self.handle_text_input_outcome(outcome)
            }
            _ => None,
        }
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        let content_width = prompt_content_width(usize::from(context.size.width));
        self.text_input.set_content_width(content_width);

        let picker_query_len = self.file_picker.as_ref().map(|picker| picker.query().len());
        let mut lines = InputPrompt {
            input: self.text_input.buffer(),
            cursor_index: self.text_input.cursor_index(picker_query_len),
        }
        .layout(context)
        .lines;

        for attachment in &self.pending_media {
            let kind = classify_attachment(&attachment.path);
            let label = match kind {
                AttachmentKind::Image => "image",
                AttachmentKind::Audio => "audio",
                _ => "file",
            };
            let mut line = Line::default();
            line.push_styled(format!("  attached {label}: {}", attachment.display_name), context.theme.info());
            lines.push(line);
        }

        if let Some(ref mut picker) = self.file_picker {
            lines.extend(picker.render(context).into_lines());
        }

        if let Some(ref mut picker) = self.command_picker {
            lines.extend(picker.render(context).into_lines());
        }

        Frame::new(lines).with_cursor(self.cursor(context))
    }
}

fn collect_submit_attachments(user_input: &str, selected_mentions: Vec<SelectedFileMention>) -> Vec<PromptAttachment> {
    let mentions: HashSet<&str> = user_input.split_whitespace().collect();
    selected_mentions
        .into_iter()
        .filter(|mention| mentions.contains(mention.mention.as_str()))
        .map(|mention| PromptAttachment { path: mention.path, display_name: mention.display_name })
        .collect()
}

fn builtin_commands() -> Vec<CommandEntry> {
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
            name: "resume".into(),
            description: "Resume a previous session".into(),
            has_input: false,
            hint: None,
            builtin: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use tui::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> Event {
        key_with_modifiers(code, KeyModifiers::NONE)
    }

    fn key_with_modifiers(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    fn cmd(name: &str, has_input: bool, builtin: bool) -> CommandEntry {
        CommandEntry {
            name: name.into(),
            description: name.into(),
            has_input,
            hint: if has_input { Some("arg".into()) } else { None },
            builtin,
        }
    }

    async fn type_chars(composer: &mut PromptComposer, chars: &str) {
        for c in chars.chars() {
            composer.on_event(&key(KeyCode::Char(c))).await;
        }
    }

    #[tokio::test]
    async fn builtin_settings_command_emits_open_settings() {
        let mut composer = PromptComposer::default();
        type_chars(&mut composer, "/").await;
        assert!(composer.has_command_picker());

        composer.on_event(&key(KeyCode::Down)).await;
        let msgs = composer.on_event(&key(KeyCode::Enter)).await.unwrap();
        assert!(matches!(msgs.as_slice(), [PromptComposerMessage::OpenSettings]));
        assert_eq!(composer.buffer(), "");
        assert!(!composer.has_active_picker());
    }

    #[tokio::test]
    async fn command_without_input_requests_submit_immediately() {
        let mut composer = PromptComposer::default();
        composer.set_available_commands(vec![cmd("status", false, false)]);
        type_chars(&mut composer, "/s").await;

        let msgs = composer.on_event(&key(KeyCode::Enter)).await.unwrap();
        assert!(matches!(
            msgs.as_slice(),
            [PromptComposerMessage::SubmitRequested { user_input, .. }] if user_input == "/status"
        ));
        assert_eq!(composer.buffer(), "");
    }

    #[tokio::test]
    async fn command_with_input_populates_prompt_without_submit() {
        let mut composer = PromptComposer::default();
        composer.set_available_commands(vec![cmd("search", true, false)]);
        type_chars(&mut composer, "/s").await;

        let msgs = composer.on_event(&key(KeyCode::Enter)).await.unwrap();
        assert!(msgs.is_empty());
        assert_eq!(composer.buffer(), "/search ");
        assert!(!composer.has_active_picker());
    }

    #[test]
    fn submit_filters_unmentioned_attachments() {
        let mut composer = PromptComposer::default();
        composer.set_input("inspect @keep.rs now".to_string());
        composer.apply_file_selection(PathBuf::from("/tmp/keep.rs"), "keep.rs".to_string());
        composer.set_input("inspect @keep.rs now @skip.rs".to_string());
        composer.apply_file_selection(PathBuf::from("/tmp/skip.rs"), "skip.rs".to_string());
        composer.set_input("inspect @keep.rs now".to_string());

        let msgs = composer.prepare_submit();
        let [PromptComposerMessage::SubmitRequested { attachments, .. }] = msgs.as_slice() else {
            panic!("expected submit request");
        };
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].display_name, "keep.rs");
        assert_eq!(attachments[0].path, PathBuf::from("/tmp/keep.rs"));
    }

    #[tokio::test]
    async fn submit_preserves_internal_hard_newlines() {
        let mut composer = PromptComposer::default();
        type_chars(&mut composer, "first").await;
        composer.on_event(&key_with_modifiers(KeyCode::Enter, KeyModifiers::SHIFT)).await;
        type_chars(&mut composer, "second").await;

        let msgs = composer.on_event(&key(KeyCode::Enter)).await.unwrap();
        assert!(matches!(
            msgs.as_slice(),
            [PromptComposerMessage::SubmitRequested { user_input, .. }] if user_input == "first\nsecond"
        ));
    }

    #[tokio::test]
    async fn shift_enter_closes_command_picker_and_inserts_newline() {
        let mut composer = PromptComposer::default();
        type_chars(&mut composer, "/").await;
        assert!(composer.has_command_picker());

        let msgs = composer.on_event(&key_with_modifiers(KeyCode::Enter, KeyModifiers::SHIFT)).await.unwrap();
        assert!(msgs.is_empty());
        assert!(!composer.has_command_picker());
        assert_eq!(composer.buffer(), "/\n");
    }

    #[tokio::test]
    async fn paste_closes_picker_and_inserts_text() {
        let mut composer = PromptComposer::default();
        type_chars(&mut composer, "@").await;
        assert!(composer.has_file_picker());

        let msgs = composer.on_event(&Event::Paste("pasted text".into())).await.unwrap();
        assert!(msgs.is_empty());
        assert!(!composer.has_active_picker());
        assert_eq!(composer.buffer(), "@pasted text");
    }

    #[tokio::test]
    async fn file_picker_cursor_tracks_query_length() {
        let mut composer = PromptComposer::default();
        type_chars(&mut composer, "@fo").await;
        assert_eq!(composer.cursor_index(), 3);
    }

    #[test]
    fn command_picker_cursor_stays_in_prompt_row() {
        let mut composer = PromptComposer::default();
        composer.open_command_picker_with_entries(vec![cmd("settings", false, true)]);

        let context = ViewContext::new((120, 40));
        let output = composer.render(&context);
        let cursor = composer.cursor(&context);
        let input_row =
            output.lines().iter().position(|line| line.plain_text().contains("> ")).expect("input prompt should exist");
        assert_eq!(cursor.row, input_row);
    }

    fn create_temp_media(dir: &TempDir, name: &str) -> PathBuf {
        let p = dir.path().join(name);
        std::fs::write(&p, b"fake media data").unwrap();
        p
    }

    #[tokio::test]
    async fn paste_image_path_adds_pending_media_attachment() {
        let tmp = TempDir::new().unwrap();
        let img = create_temp_media(&tmp, "photo.png");

        let mut composer = PromptComposer::default();
        composer.on_event(&Event::Paste(img.to_str().unwrap().into())).await;

        assert_eq!(composer.pending_media().len(), 1);
        assert_eq!(composer.pending_media()[0].display_name, "photo.png");
        assert_eq!(composer.buffer(), "");
    }

    #[tokio::test]
    async fn paste_audio_path_adds_pending_media_attachment() {
        let tmp = TempDir::new().unwrap();
        let audio = create_temp_media(&tmp, "note.wav");

        let mut composer = PromptComposer::default();
        composer.on_event(&Event::Paste(audio.to_str().unwrap().into())).await;

        assert_eq!(composer.pending_media().len(), 1);
        assert_eq!(composer.pending_media()[0].display_name, "note.wav");
        assert_eq!(composer.buffer(), "");
    }

    #[tokio::test]
    async fn paste_ordinary_text_inserts_into_prompt() {
        let mut composer = PromptComposer::default();
        composer.on_event(&Event::Paste("hello world".into())).await;

        assert!(composer.pending_media().is_empty());
        assert_eq!(composer.buffer(), "hello world");
    }

    #[tokio::test]
    async fn paste_non_media_file_falls_back_to_text() {
        let tmp = TempDir::new().unwrap();
        let txt = create_temp_media(&tmp, "readme.txt");

        let mut composer = PromptComposer::default();
        composer.on_event(&Event::Paste(txt.to_str().unwrap().into())).await;

        // Text files are not media — should fall back to inserting the path as text
        assert!(composer.pending_media().is_empty());
        assert!(!composer.buffer().is_empty());
    }

    #[tokio::test]
    async fn paste_closes_file_picker_before_processing_drop() {
        let tmp = TempDir::new().unwrap();
        let img = create_temp_media(&tmp, "screen.png");

        let mut composer = PromptComposer::default();
        type_chars(&mut composer, "@").await;
        assert!(composer.has_file_picker());

        composer.on_event(&Event::Paste(img.to_str().unwrap().into())).await;

        assert!(!composer.has_active_picker());
        assert_eq!(composer.pending_media().len(), 1);
    }

    #[tokio::test]
    async fn duplicate_dropped_file_is_not_added_twice() {
        let tmp = TempDir::new().unwrap();
        let img = create_temp_media(&tmp, "photo.png");
        let path_str = img.to_str().unwrap().to_string();

        let mut composer = PromptComposer::default();
        composer.on_event(&Event::Paste(path_str.clone())).await;
        composer.on_event(&Event::Paste(path_str)).await;

        assert_eq!(composer.pending_media().len(), 1);
    }

    #[tokio::test]
    async fn submit_merges_dropped_media_with_prompt() {
        let tmp = TempDir::new().unwrap();
        let img = create_temp_media(&tmp, "photo.png");

        let mut composer = PromptComposer::default();
        composer.on_event(&Event::Paste(img.to_str().unwrap().into())).await;
        type_chars(&mut composer, "describe this").await;

        let msgs = composer.on_event(&key(KeyCode::Enter)).await.unwrap();
        let [PromptComposerMessage::SubmitRequested { user_input, attachments }] = msgs.as_slice() else {
            panic!("expected submit request");
        };
        assert_eq!(user_input, "describe this");
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].display_name, "photo.png");
    }

    #[tokio::test]
    async fn submit_clears_pending_media() {
        let tmp = TempDir::new().unwrap();
        let img = create_temp_media(&tmp, "photo.png");

        let mut composer = PromptComposer::default();
        composer.on_event(&Event::Paste(img.to_str().unwrap().into())).await;
        type_chars(&mut composer, "go").await;
        composer.on_event(&key(KeyCode::Enter)).await;

        assert!(composer.pending_media().is_empty());
        assert_eq!(composer.buffer(), "");
    }

    #[tokio::test]
    async fn submit_with_only_media_and_no_text() {
        let tmp = TempDir::new().unwrap();
        let img = create_temp_media(&tmp, "photo.png");

        let mut composer = PromptComposer::default();
        composer.on_event(&Event::Paste(img.to_str().unwrap().into())).await;

        let msgs = composer.on_event(&key(KeyCode::Enter)).await.unwrap();
        let [PromptComposerMessage::SubmitRequested { user_input, attachments }] = msgs.as_slice() else {
            panic!("expected submit request");
        };
        assert_eq!(user_input, "");
        assert_eq!(attachments.len(), 1);
    }

    #[tokio::test]
    async fn backspace_on_empty_removes_last_dropped_media() {
        let tmp = TempDir::new().unwrap();
        let img1 = create_temp_media(&tmp, "a.png");
        let img2 = create_temp_media(&tmp, "b.png");

        let mut composer = PromptComposer::default();
        composer.on_event(&Event::Paste(img1.to_str().unwrap().into())).await;
        composer.on_event(&Event::Paste(img2.to_str().unwrap().into())).await;
        assert_eq!(composer.pending_media().len(), 2);

        composer.on_event(&key(KeyCode::Backspace)).await;
        assert_eq!(composer.pending_media().len(), 1);
        assert_eq!(composer.pending_media()[0].display_name, "a.png");

        composer.on_event(&key(KeyCode::Backspace)).await;
        assert!(composer.pending_media().is_empty());
    }

    #[test]
    fn render_shows_attachment_tray() {
        let tmp = TempDir::new().unwrap();
        let img = create_temp_media(&tmp, "photo.png");

        let mut composer = PromptComposer::default();
        composer.pending_media.push(PromptAttachment { path: img, display_name: "photo.png".to_string() });

        let context = ViewContext::new((80, 24));
        let output = composer.render(&context);
        let text: String = output.lines().iter().map(tui::Line::plain_text).collect::<Vec<_>>().join("\n");
        assert!(text.contains("attached image: photo.png"));
    }

    #[test]
    fn render_shows_multiple_attachments() {
        let tmp = TempDir::new().unwrap();
        let img = create_temp_media(&tmp, "photo.png");
        let audio = create_temp_media(&tmp, "note.wav");

        let mut composer = PromptComposer::default();
        composer.pending_media.push(PromptAttachment { path: img, display_name: "photo.png".to_string() });
        composer.pending_media.push(PromptAttachment { path: audio, display_name: "note.wav".to_string() });

        let context = ViewContext::new((80, 24));
        let output = composer.render(&context);
        let text: String = output.lines().iter().map(tui::Line::plain_text).collect::<Vec<_>>().join("\n");
        assert!(text.contains("attached image: photo.png"));
        assert!(text.contains("attached audio: note.wav"));
    }

    #[tokio::test]
    async fn submit_records_prompt_in_history() {
        let mut composer = PromptComposer::default();
        type_chars(&mut composer, "hello").await;
        composer.on_event(&key(KeyCode::Enter)).await;

        type_chars(&mut composer, "world").await;
        composer.on_event(&key(KeyCode::Enter)).await;

        composer.on_event(&key(KeyCode::Up)).await;
        assert_eq!(composer.buffer(), "world");

        composer.on_event(&key(KeyCode::Up)).await;
        assert_eq!(composer.buffer(), "hello");
    }

    #[tokio::test]
    async fn submit_with_only_media_does_not_record_empty_text() {
        let tmp = TempDir::new().unwrap();
        let img = create_temp_media(&tmp, "photo.png");

        let mut composer = PromptComposer::default();
        composer.on_event(&Event::Paste(img.to_str().unwrap().into())).await;
        composer.on_event(&key(KeyCode::Enter)).await;

        composer.on_event(&key(KeyCode::Up)).await;
        assert_eq!(composer.buffer(), "");
    }
}
