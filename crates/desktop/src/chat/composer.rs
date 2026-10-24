use super::attachments::{PastedImages, attachment_chip};
use super::completion::{ActiveToken, CompletionItem, CompletionMenu, Trigger};
use super::footer::{config_selector, context_meter, queued_prompts};
use super::prompt_search::{PromptSearch, PromptSearchOutcome};
use crate::file_index::FileIndex;
use crate::theme::DesktopTheme;
use crate::threads::{Thread, ThreadStatus};
use crate::ui::tokens::{CONTENT_MAX_WIDTH, GROUP_HEIGHT, rem};
use crate::ui::{IconButton, IconName, PickerItem, SearchPicker, SearchPickerEvent, SurfaceExt};
use client_core::composer::attachment::{AttachmentKind, PromptAttachment, classify_attachment};
use client_core::composer::prompt_history::PromptHistory;
use client_core::composer::submission::Submission;
use client_core::session_core::SessionCore;
use client_core::workspace_status::WorkspaceAccess;
use gpui::{
    App, ClickEvent, ClipboardEntry, ClipboardItem, Context, Div, Entity, Focusable, MouseButton, PathPromptOptions,
    SharedString, Stateful, Subscription, Window, actions, div, prelude::*,
};
use gpui_base::input::{
    Backspace, Enter, Escape, IndentInline, InlineToken, InputContent, InputEvent, MoveDown, MoveUp, Paste, Textarea,
    TextareaState,
};
use std::path::{Path, PathBuf};

actions!(desktop, [SearchPrompts]);

/// Where the user writes the next prompt for a thread, with a Stop button while its turn runs.
///
/// Enter and ⌘/Ctrl+Enter send, Shift+Enter starts a new line. In an empty composer ↑ and ↓
/// walk back through the prompts sent from it. Typing `/` at the start offers the agent's
/// commands, and `@` offers the project's files, which the prompt then carries. ⌘/Ctrl+R
/// searches every prompt the agent has been sent.
///
/// Below the text sit the session's options, such as its model, and how full the context is.
pub(crate) struct Composer {
    thread: Entity<Thread>,
    input: Entity<TextareaState>,
    history: PromptHistory,
    /// Whether the text is a prompt recalled from history, untouched since.
    recalling: bool,
    completion: Option<CompletionMenu>,
    /// Where the token starts whose menu the user dismissed, so it stays shut while they type on.
    dismissed: Option<usize>,
    search: Option<(Entity<PromptSearch>, Subscription)>,
    config_picker: Option<OpenPicker>,
    media: Vec<PromptAttachment>,
    pasted: PastedImages,
    _subscriptions: Vec<Subscription>,
}

impl Composer {
    pub(crate) fn new(thread: Entity<Thread>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let style = DesktopTheme::global(cx).ui().editor_style();
        let input = cx.new(|cx| {
            let mut input = TextareaState::new(window, cx)
                .auto_grow(MIN_ROWS, MAX_ROWS)
                .submit_on_enter(true)
                .placeholder("Message the agent…  / for commands, @ for files");
            input.set_editor_style(style);
            input
        });
        let subscriptions = vec![
            cx.observe_in(&thread, window, |this, _, window, cx| {
                this.restore_submissions(window, cx);
                cx.notify();
            }),
            cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.history.reset();
                    this.recalling = false;
                    this.refresh_completion(cx);
                    cx.notify();
                }
            }),
            cx.observe_global::<FileIndex>(|this, cx| {
                if this.completion.as_ref().is_some_and(|menu| menu.token().trigger == Trigger::File) {
                    this.refresh_completion(cx);
                }
            }),
            cx.observe_global::<DesktopTheme>(|this, cx| {
                let style = DesktopTheme::global(cx).ui().editor_style();
                this.input.update(cx, |input, cx| {
                    input.set_editor_style(style);
                    cx.notify();
                });
            }),
        ];
        Self {
            thread,
            input,
            history: PromptHistory::default(),
            recalling: false,
            completion: None,
            dismissed: None,
            search: None,
            config_picker: None,
            media: Vec::new(),
            pasted: PastedImages::default(),
            _subscriptions: subscriptions,
        }
    }

    pub(crate) fn text(&self, cx: &App) -> SharedString {
        self.input.read(cx).value()
    }

    pub(crate) fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    pub(crate) fn attach(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let remote = self
            .thread
            .read(cx)
            .core()
            .is_some_and(|core| core.session().workspace_access() == WorkspaceAccess::Remote);
        let root = self.root(cx);
        let mut refused = Vec::new();
        for path in paths {
            let attachment = PromptAttachment { path: path.clone(), display_name: display_name(path, &root) };
            match path.is_file().then(|| classify_attachment(path)) {
                Some(AttachmentKind::Image | AttachmentKind::Audio) => self.add_media(attachment),
                Some(AttachmentKind::Text) => self.input.update(cx, |input, cx| {
                    let mention = InlineToken::new(path.to_string_lossy().into_owned(), mention_text(&attachment));
                    if input.replace_with_token(mention, window, cx).is_ok() {
                        input.insert(" ", window, cx);
                    }
                }),
                _ if remote => refused.push(attachment.display_name),
                _ => self.input.update(cx, |input, cx| input.insert(format!("{} ", path.display()), window, cx)),
            }
        }
        if !refused.is_empty() {
            let message = format!("Can't attach folders or binary files to a remote agent: {}", refused.join(", "));
            self.thread.update(cx, |thread, cx| thread.notice(&message, cx));
        }
        cx.notify();
    }

    /// Completes the token being typed with the menu's entry at `position`: a file becomes a
    /// mention, and a command is sent at once unless it takes input.
    pub(crate) fn accept_completion(&mut self, position: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.completion.take() else {
            return;
        };
        let Some(item) = menu.item(position).cloned() else {
            return;
        };
        let range = menu.token().range.clone();
        match item {
            CompletionItem::File(file) => self.input.update(cx, |input, cx| {
                let mention = InlineToken::new(file.path.to_string_lossy().into_owned(), mention_text(&file));
                if input.replace_range_with_token(range, mention, window, cx).is_ok() {
                    input.insert(" ", window, cx);
                }
            }),
            CompletionItem::Command(command) => {
                self.input.update(cx, |input, cx| {
                    input.set_selected_range(range, cx);
                    input.replace(format!("/{}", command.name), window, cx);
                    if command.has_input {
                        input.insert(" ", window, cx);
                    }
                });
                if !command.has_input {
                    self.send(window, cx);
                }
            }
        }
        cx.notify();
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let content = self.input.read(cx).content();
        let text = content.text().to_string();
        if text.trim().is_empty() && self.media.is_empty() {
            return;
        }
        let submission = Submission { text, mentions: mentions(&content), media: std::mem::take(&mut self.media) };
        self.history.push(&submission.text);
        self.history.reset();
        self.recalling = false;
        self.completion = None;
        self.input.update(cx, |input, cx| input.clean(window, cx));
        self.thread.update(cx, |thread, cx| thread.send(submission, cx));
    }

    fn add_media(&mut self, attachment: PromptAttachment) {
        if self.media.iter().all(|media| media.path != attachment.path) {
            self.media.push(attachment);
        }
    }

    fn attach_clipboard(&mut self, clipboard: &ClipboardItem, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let mut attached = false;
        for entry in &clipboard.entries {
            match entry {
                ClipboardEntry::ExternalPaths(paths) => self.attach(paths.paths(), window, cx),
                ClipboardEntry::Image(image) => match self.pasted.save(image) {
                    Ok(attachment) => self.add_media(attachment),
                    Err(message) => self.thread.update(cx, |thread, cx| thread.notice(&message, cx)),
                },
                ClipboardEntry::String(_) => continue,
            }
            attached = true;
        }
        cx.notify();
        attached
    }

    fn pick_files(window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(paths) = picked.await.ok().and_then(Result::ok).flatten() else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.attach(&paths, window, cx);
                this.focus(window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Opens or closes the completion menu for the token the cursor ends.
    fn refresh_completion(&mut self, cx: &mut Context<Self>) {
        let input = self.input.read(cx);
        let text = input.value();
        let token = ActiveToken::at(&text, input.cursor(), input.tokens());
        let Some(token) = token else {
            self.completion = None;
            self.dismissed = None;
            cx.notify();
            return;
        };
        if self.dismissed == Some(token.range.start) {
            return;
        }
        let was_open = self.completion.as_ref().map(|menu| (menu.token().trigger, menu.token().range.start));
        self.completion = match token.trigger {
            Trigger::Command => {
                let thread = self.thread.read(cx);
                let commands = thread.core().map(SessionCore::available_commands).unwrap_or_default();
                (!commands.is_empty()).then(|| CompletionMenu::commands(token, &text, commands))
            }
            Trigger::File => {
                let root = self.root(cx);
                if was_open != Some((Trigger::File, token.range.start)) {
                    FileIndex::refresh(&root, cx);
                }
                Some(CompletionMenu::files(token, &text, FileIndex::files(&root, cx).as_deref()))
            }
        };
        cx.notify();
    }

    /// The directory `@` mentions are relative to.
    fn root(&self, cx: &App) -> PathBuf {
        self.thread.read(cx).target().cwd().to_path_buf()
    }

    fn running(&self, cx: &App) -> bool {
        matches!(self.thread.read(cx).status(), ThreadStatus::Running | ThreadStatus::NeedsInput)
    }

    fn dismiss_completion(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = self.completion.take() {
            self.dismissed = Some(menu.token().range.start);
            cx.notify();
        }
    }

    /// Replaces an empty composer, or a prompt recalled into it, with the one sent before it.
    fn recall_older(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let text = self.text(cx);
        if !text.is_empty() && !self.recalling {
            return false;
        }
        let Some(prompt) = self.history.older(&text).map(str::to_owned) else {
            return false;
        };
        self.recall(prompt, window, cx);
        true
    }

    /// Steps forward through recalled prompts, back to the draft they replaced.
    fn recall_newer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.recalling {
            return false;
        }
        let Some(prompt) = self.history.newer() else {
            return false;
        };
        self.recall(prompt, window, cx);
        true
    }

    /// Shows `prompt` without counting it as an edit, which would end the walk through history.
    fn recall(&mut self, prompt: String, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.set_value(prompt, window, cx));
        self.recalling = true;
        self.completion = None;
        cx.notify();
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.is_some() || !self.thread.read(cx).can_search_prompts() {
            return;
        }
        let search = cx.new(|cx| PromptSearch::new(self.thread.clone(), window, cx));
        let subscription = cx.subscribe_in(&search, window, |this, _, outcome: &PromptSearchOutcome, window, cx| {
            if let PromptSearchOutcome::Chose(prompt) = outcome {
                this.input.update(cx, |input, cx| input.set_value(prompt.clone(), window, cx));
            }
            this.search = None;
            this.focus(window, cx);
            cx.notify();
        });
        self.search = Some((search, subscription));
        self.completion = None;
        cx.notify();
    }

    /// Opens the picker for the session option `option_id`, closing any other.
    fn open_config_picker(&mut self, option_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(option) = self
            .thread
            .read(cx)
            .core()
            .and_then(|core| core.session().config_options().iter().find(|option| option.id == option_id).cloned())
        else {
            return;
        };
        let Some(select) = option.select() else {
            return;
        };
        let values = select.values.to_vec();
        let current = values.iter().position(|value| value.value == select.current_value).unwrap_or(0);
        let items = values
            .iter()
            .map(|value| {
                let item = PickerItem::new(value.value.clone(), value.name.clone())
                    .keywords(value.value.clone())
                    .disabled(value.is_disabled)
                    .checked(value.value == select.current_value);
                if value.is_disabled { item.detail("unavailable") } else { item }
            })
            .collect();
        let theme = DesktopTheme::global(cx).ui();
        let picker = cx.new(|cx| {
            let picker = SearchPicker::new(format!("config-{}-picker", option.id), items, theme, cx)
                .title(option.name.clone())
                .width(rem(CONFIG_PICKER_WIDTH))
                .max_list_height(rem(CONFIG_LIST_MAX_HEIGHT))
                .highlight(current);
            if values.len() > CONFIG_SEARCH_THRESHOLD {
                picker.searchable(format!("Search {}…", option.name.to_lowercase()), window, cx)
            } else {
                picker
            }
        });
        let subscription = cx.subscribe_in(&picker, window, move |this, _, event: &SearchPickerEvent, window, cx| {
            match event {
                SearchPickerEvent::Chose(index) => {
                    let value = &values[*index].value;
                    this.thread.update(cx, |thread, cx| thread.set_config_option(&option_id, value, cx));
                }
                SearchPickerEvent::Dismissed => {}
                SearchPickerEvent::Query(_) => return,
            }
            this.close_config_picker(window, cx);
        });
        self.config_picker = Some(OpenPicker { option_id: option.id.clone(), picker, _subscription: subscription });
        self.completion = None;
        cx.notify();
    }

    fn close_config_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.config_picker.take().is_some() {
            self.focus(window, cx);
            cx.notify();
        }
    }

    /// Gives back a prompt the agent never received, below anything written since.
    fn restore_submissions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let submissions = self.thread.update(cx, |thread, _| thread.take_restored_submissions());
        if submissions.is_empty() {
            return;
        }
        let draft = self.input.read(cx).content();
        let content = submissions.iter().fold(draft, |draft, submission| restored_content(&draft, submission));
        self.input.update(cx, |input, cx| input.set_value(content, window, cx));
        for attachment in submissions.into_iter().flat_map(|submission| submission.media) {
            self.add_media(attachment);
        }
    }

    fn render_input(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("composer-input")
            .w_full()
            .min_w_0()
            .px_3()
            .pt_2()
            .cursor_text()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.focus(window, cx);
                    cx.stop_propagation();
                }),
            )
            // Captured on the way down to the textarea, which would otherwise move its cursor.
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
                if let Some(menu) = &mut this.completion {
                    menu.step(-1);
                    cx.notify();
                } else if !this.recall_older(window, cx) {
                    return;
                }
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                if let Some(menu) = &mut this.completion {
                    menu.step(1);
                    cx.notify();
                } else if !this.recall_newer(window, cx) {
                    return;
                }
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                if let Some(menu) = &this.completion {
                    this.accept_completion(menu.highlighted(), window, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &Paste, window, cx| {
                if let Some(clipboard) = cx.read_from_clipboard()
                    && this.attach_clipboard(&clipboard, window, cx)
                {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &Backspace, _, cx| {
                if this.text(cx).is_empty() && this.media.pop().is_some() {
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                if this.completion.is_some() {
                    this.dismiss_completion(cx);
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|this, _: &Escape, _, cx| {
                if this.running(cx) {
                    this.thread.update(cx, Thread::cancel);
                } else {
                    cx.propagate();
                }
            }))
            // The textarea inserts Shift+Enter's newline itself and passes every other Enter up
            // here. Handling it marks the keystroke handled, so it is not also typed as a newline.
            .on_action(cx.listener(|this, enter: &Enter, window, cx| {
                if enter.shift {
                    return;
                }
                match &this.completion {
                    Some(menu) if menu.item(menu.highlighted()).is_some() => {
                        this.accept_completion(menu.highlighted(), window, cx);
                    }
                    _ => this.send(window, cx),
                }
            }))
            .on_action(cx.listener(|this, _: &SearchPrompts, window, cx| this.open_search(window, cx)))
            .child(Textarea::new(&self.input))
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> Div {
        let theme = DesktopTheme::global(cx).ui();
        let footer = self.thread.read(cx).core().map(|core| {
            let selectors: Vec<_> = core
                .session()
                .config_options()
                .iter()
                .filter_map(|option| {
                    let picker = self
                        .config_picker
                        .as_ref()
                        .filter(|open| open.option_id == option.id)
                        .map(|open| open.picker.clone());
                    let option_id = option.id.clone();
                    let on_open_change = cx.listener(move |this, open: &bool, window, cx| {
                        if *open {
                            this.open_config_picker(option_id.clone(), window, cx);
                        } else {
                            this.close_config_picker(window, cx);
                        }
                    });
                    config_selector(option, picker, &theme, on_open_change)
                })
                .collect();
            let meter = core.conversation().context_usage().map(|usage| context_meter(usage, &theme));
            div()
                .debug_selector(|| "composer-footer".to_owned())
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap_1()
                .children(selectors)
                .child(div().flex_1())
                .children(meter)
        });
        let attach = IconButton::new("attach-files", IconName::Paperclip, "Attach files", theme)
            .on_click(cx.listener(|_, _: &ClickEvent, window, cx| Self::pick_files(window, cx)));
        let action = if self.running(cx) && self.text(cx).trim().is_empty() && self.media.is_empty() {
            IconButton::new("stop", IconName::Square, "Stop (Esc)", theme)
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.thread.update(cx, Thread::cancel)))
        } else {
            IconButton::new("send", IconName::ArrowUp, "Send", theme)
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.send(window, cx)))
        };
        div()
            .h(rem(GROUP_HEIGHT))
            .px_1()
            .mb_1()
            .flex()
            .items_center()
            .gap_2()
            .children(footer)
            .when(self.thread.read(cx).core().is_none(), |toolbar| toolbar.child(div().flex_1()))
            .child(attach)
            .child(action)
    }
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = DesktopTheme::global(cx).ui();
        let focused = self.input.focus_handle(cx).is_focused(window);
        let queued = self
            .thread
            .read(cx)
            .core()
            .map(SessionCore::queued_prompts)
            .filter(|prompts| !prompts.is_empty())
            .map(|prompts| queued_prompts(prompts, &theme));
        let completion = self.completion.as_ref().map(|menu| menu.render(&theme, cx));
        let search = self.search.as_ref().map(|(search, _)| search.clone());
        let chips: Vec<_> = self
            .media
            .iter()
            .enumerate()
            .map(|(index, attachment)| {
                let remove = cx.listener(move |this, _: &ClickEvent, window, cx| {
                    if index < this.media.len() {
                        this.media.remove(index);
                        this.focus(window, cx);
                        cx.notify();
                    }
                });
                attachment_chip(index, attachment, &theme, remove)
            })
            .collect();
        let attachments = (!chips.is_empty()).then(|| div().px_3().pt_2().flex().flex_wrap().gap_1().children(chips));
        let editor = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .surface(theme)
            .when(focused, |editor| editor.border_color(theme.colors.accent))
            .children(attachments)
            .child(self.render_input(cx))
            .child(self.render_toolbar(cx));
        div().debug_selector(|| "composer".to_owned()).flex_none().w_full().px_4().pt_2().pb_4().child(
            div()
                .w_full()
                .max_w(rem(CONTENT_MAX_WIDTH))
                .mx_auto()
                .flex()
                .flex_col()
                .gap_2()
                .children(queued)
                .children(search)
                .children(completion)
                .child(editor),
        )
    }
}

struct OpenPicker {
    option_id: String,
    picker: Entity<SearchPicker>,
    _subscription: Subscription,
}

const MIN_ROWS: usize = 1;
const MAX_ROWS: usize = 12;
/// Options with more values than this can be searched.
const CONFIG_SEARCH_THRESHOLD: usize = 8;
const CONFIG_PICKER_WIDTH: f32 = 288.0;
/// Tall enough for ten rows, short enough to fit above the composer in the smallest window.
const CONFIG_LIST_MAX_HEIGHT: f32 = 288.0;

fn mention_text(file: &PromptAttachment) -> String {
    format!("@{}", file.display_name)
}

/// The files the prompt's mentions name, each once.
fn mentions(content: &InputContent) -> Vec<PromptAttachment> {
    let mut mentions: Vec<PromptAttachment> = Vec::new();
    for span in content.tokens() {
        let token = span.token();
        let path = PathBuf::from(token.id().as_ref());
        if mentions.iter().all(|mention| mention.path != path) {
            let display_name = token.text().strip_prefix('@').unwrap_or(token.text()).to_owned();
            mentions.push(PromptAttachment { path, display_name });
        }
    }
    mentions
}

fn display_name(path: &Path, root: &Path) -> String {
    let name = path.strip_prefix(root).ok().or_else(|| path.file_name().map(Path::new)).unwrap_or(path);
    name.to_string_lossy().replace('\\', "/")
}

/// `draft` with `submission` below it, its mentions shown as mentions again.
fn restored_content(draft: &InputContent, submission: &Submission) -> InputContent {
    if submission.text.is_empty() {
        return draft.clone();
    }
    let offset = if draft.text().is_empty() { 0 } else { draft.text().len() + 1 };
    let text = if draft.text().is_empty() {
        submission.text.clone()
    } else {
        format!("{}\n{}", draft.text(), submission.text)
    };
    let mut content = InputContent::new(text.clone());
    for span in draft.tokens() {
        content = content.clone().with_token(span.range(), span.token().clone()).unwrap_or(content);
    }
    for mention in &submission.mentions {
        let needle = mention_text(mention);
        for (start, _) in submission.text.match_indices(&needle) {
            let range = offset + start..offset + start + needle.len();
            let token = InlineToken::new(mention.path.to_string_lossy().into_owned(), needle.clone());
            content = content.clone().with_token(range, token).unwrap_or(content);
        }
    }
    content
}
