mod completion;
mod pasted;
mod prompt_search;
mod queued;

use super::attachment::{chip, preview};
use crate::file_index::FileIndex;
use crate::theme::DesktopTheme;
use crate::threads::{Thread, ThreadEvent, observe_thread};
use crate::ui::tokens::{ICON_SIZE, ROW_HEIGHT, rem};
use crate::ui::{Icon, IconButton, IconName, SurfaceExt, Tooltip, Typography, TypographyExt, UiTheme, text_selector};
use client_core::composer::attachment::{AttachedFile, PromptAttachment, classify_paths};
use client_core::composer::completion_token::Trigger;
use client_core::composer::prompt_history::PromptHistory;
use client_core::composer::submission::Submission;
use client_core::session_core::SessionCore;
use client_core::session_model::WorkspaceAccess;
use completion::{CompletionItem, CompletionMenu, active_token};
use gpui::{
    App, ClickEvent, ClipboardEntry, ClipboardItem, Context, Div, Entity, Focusable, MouseButton, PathPromptOptions,
    Role, SharedString, Stateful, Subscription, Window, actions, div, prelude::*,
};
use gpui_base::input::{
    Backspace, Enter, Escape, IndentInline, InlineToken, InputContent, InputEditorStyle, InputEvent, MoveDown, MoveUp,
    Paste, Textarea, TextareaState,
};
use pasted::PastedImages;
use prompt_search::{PromptSearch, PromptSearchOutcome};
use queued::queued_prompts;
use std::ops::Range;
use std::path::{Path, PathBuf};

actions!(desktop, [SearchPrompts]);

pub(crate) struct Composer {
    thread: Entity<Thread>,
    input: Entity<TextareaState>,
    history: PromptHistory,
    popup: Option<Popup>,
    media: Vec<PromptAttachment>,
    pasted: PastedImages,
    _subscriptions: Vec<Subscription>,
}

impl Composer {
    pub(crate) fn new(thread: Entity<Thread>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let style = editor_style(&DesktopTheme::global(cx).ui());
        let input = cx.new(|cx| {
            let mut input = TextareaState::new(window, cx)
                .auto_grow(MIN_ROWS, MAX_ROWS)
                .submit_on_enter(true)
                .placeholder("Message the agent…  / for commands, @ for files");
            input.set_editor_style(style);
            input
        });
        let subscriptions = vec![
            observe_thread(
                &thread,
                cx,
                |thread| {
                    let core = thread.core();
                    (
                        thread.status(),
                        core.map(|core| {
                            core.queued_prompts().iter().map(|prompt| prompt.request_id).collect::<Vec<_>>()
                        }),
                    )
                },
                |_, cx| cx.notify(),
            ),
            cx.subscribe_in(&thread, window, |this, _, event: &ThreadEvent, window, cx| {
                if let ThreadEvent::SubmissionRestored(submission) = event {
                    this.restore(submission, window, cx);
                }
            }),
            cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.history.reset();
                    this.refresh_completion(cx);
                    cx.notify();
                }
            }),
            cx.observe_global::<FileIndex>(|this, cx| {
                if this.completion().is_some_and(|menu| menu.token().trigger == Trigger::File) {
                    this.refresh_completion(cx);
                }
            }),
            cx.observe_global::<DesktopTheme>(|this, cx| {
                let style = editor_style(&DesktopTheme::global(cx).ui());
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
            popup: None,
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
        let mut refused = Vec::new();
        for file in classify_paths(paths.iter().cloned(), &self.root(cx).unwrap_or_default()) {
            match file {
                AttachedFile::Media(media) => self.add_media(media),
                AttachedFile::Mention(mention) => self.input.update(cx, |input, cx| {
                    let selection = input.selected_range();
                    insert_mention(input, selection, &mention, window, cx);
                }),
                AttachedFile::Unsupported(file) if remote => refused.push(file.display_name),
                AttachedFile::Unsupported(file) => {
                    self.input.update(cx, |input, cx| input.insert(format!("{} ", file.path.display()), window, cx));
                }
            }
        }
        if !refused.is_empty() {
            let message = format!("Can't attach folders or binary files to a remote agent: {}", refused.join(", "));
            self.thread.update(cx, |thread, cx| thread.report_error(&message, cx));
        }
        cx.notify();
    }

    pub(crate) fn accept_completion(&mut self, position: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Popup::Completion(menu)) = self.popup.take() else {
            return;
        };
        let Some(item) = menu.item(position).cloned() else {
            return;
        };
        let range = menu.token().range.clone();
        match item {
            CompletionItem::File(file) => {
                self.input.update(cx, |input, cx| insert_mention(input, range, &file, window, cx));
            }
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
        self.close_completion();
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
                    Err(message) => self.thread.update(cx, |thread, cx| thread.report_error(&message, cx)),
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

    fn refresh_completion(&mut self, cx: &mut Context<Self>) {
        let input = self.input.read(cx);
        let text = input.value();
        let token = active_token(&text, input.cursor(), input.tokens());
        let Some(token) = token else {
            self.close_completion();
            cx.notify();
            return;
        };
        if matches!(self.popup, Some(Popup::Dismissed { token_start }) if token_start == token.range.start) {
            return;
        }
        let was_open = self.completion().map(|menu| (menu.token().trigger, menu.token().range.start));
        let menu = match token.trigger {
            Trigger::Command => {
                let thread = self.thread.read(cx);
                let commands = thread.core().map(SessionCore::available_commands).unwrap_or_default();
                (!commands.is_empty()).then(|| CompletionMenu::commands(token, &text, commands))
            }
            Trigger::File => self.root(cx).map(|root| {
                if was_open != Some((Trigger::File, token.range.start)) {
                    FileIndex::refresh(&root, cx);
                }
                CompletionMenu::files(token, &text, FileIndex::files(&root, cx).as_deref())
            }),
        };
        self.popup = menu.map(Popup::Completion);
        cx.notify();
    }

    fn root(&self, cx: &App) -> Option<PathBuf> {
        self.thread.read(cx).target().cwd().map(Path::to_path_buf)
    }

    fn running(&self, cx: &App) -> bool {
        self.thread.read(cx).status().is_busy()
    }

    fn cancel(&self, cx: &mut App) {
        Thread::with_core(&self.thread, cx, SessionCore::cancel);
    }

    fn completion(&self) -> Option<&CompletionMenu> {
        match &self.popup {
            Some(Popup::Completion(menu)) => Some(menu),
            _ => None,
        }
    }

    fn close_completion(&mut self) {
        if matches!(self.popup, Some(Popup::Completion(_) | Popup::Dismissed { .. })) {
            self.popup = None;
        }
    }

    fn dismiss_completion(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = self.completion() {
            self.popup = Some(Popup::Dismissed { token_start: menu.token().range.start });
            cx.notify();
        }
    }

    fn recall_older(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let text = self.text(cx);
        if !text.is_empty() && !self.history.is_recalling() {
            return false;
        }
        let Some(prompt) = self.history.older(&text).map(str::to_owned) else {
            return false;
        };
        self.recall(prompt, window, cx);
        true
    }

    fn recall_newer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.history.is_recalling() {
            return false;
        }
        let Some(prompt) = self.history.newer() else {
            return false;
        };
        self.recall(prompt, window, cx);
        true
    }

    fn step_or_recall(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let stepped = if let Some(Popup::Completion(menu)) = &mut self.popup {
            menu.step(by);
            cx.notify();
            true
        } else if by < 0 {
            self.recall_older(window, cx)
        } else {
            self.recall_newer(window, cx)
        };
        if stepped {
            cx.stop_propagation();
        }
    }

    fn recall(&mut self, prompt: String, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.set_value(prompt, window, cx));
        self.close_completion();
        cx.notify();
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.popup, Some(Popup::Search { .. })) || !self.thread.read(cx).can_search_prompts() {
            return;
        }
        let search = cx.new(|cx| PromptSearch::new(self.thread.clone(), window, cx));
        let subscription = cx.subscribe_in(&search, window, |this, _, outcome: &PromptSearchOutcome, window, cx| {
            if let PromptSearchOutcome::Chose(prompt) = outcome {
                this.input.update(cx, |input, cx| input.set_value(prompt.clone(), window, cx));
            }
            this.popup = None;
            this.focus(window, cx);
            cx.notify();
        });
        self.popup = Some(Popup::Search { search, _subscription: subscription });
        cx.notify();
    }

    fn restore(&mut self, submission: &Submission, window: &mut Window, cx: &mut Context<Self>) {
        let content = restored_content(&self.input.read(cx).content(), submission);
        self.input.update(cx, |input, cx| input.set_value(content, window, cx));
        for attachment in &submission.media {
            self.add_media(attachment.clone());
        }
        cx.notify();
    }

    fn render_input(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("composer-input")
            .w_full()
            .min_w_0()
            .px_3()
            .pt_2()
            .typography(Typography::Reading)
            .cursor_text()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.focus(window, cx);
                    cx.stop_propagation();
                }),
            )
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| this.step_or_recall(-1, window, cx)))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| this.step_or_recall(1, window, cx)))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                if let Some(highlighted) = this.completion().map(CompletionMenu::highlighted) {
                    this.accept_completion(highlighted, window, cx);
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
                if this.completion().is_some() {
                    this.dismiss_completion(cx);
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|this, _: &Escape, _, cx| {
                if this.running(cx) {
                    this.cancel(cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, enter: &Enter, window, cx| {
                if enter.shift {
                    return;
                }
                match this.completion().filter(|menu| menu.item(menu.highlighted()).is_some()) {
                    Some(menu) => this.accept_completion(menu.highlighted(), window, cx),
                    None => this.send(window, cx),
                }
            }))
            .on_action(cx.listener(|this, _: &SearchPrompts, window, cx| this.open_search(window, cx)))
            .child(Textarea::new(&self.input))
    }

    fn render_actions(&self, cx: &mut Context<Self>) -> Div {
        let theme = DesktopTheme::global(cx).ui();
        let attach = IconButton::new("attach-files", IconName::Paperclip, "Attach files", theme)
            .on_click(cx.listener(|_, _: &ClickEvent, window, cx| Self::pick_files(window, cx)));
        let empty = self.text(cx).trim().is_empty() && self.media.is_empty();
        let action = if self.running(cx) && empty {
            round_button("stop", IconName::Square, "Stop (Esc)", true, &theme)
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.cancel(cx)))
        } else {
            round_button("send", IconName::ArrowUp, "Send", !empty, &theme)
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.send(window, cx)))
        };
        div().px_2().pb_2().flex().items_center().justify_between().child(attach).child(action)
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
        let completion = self.completion().map(|menu| menu.render(&theme, cx));
        let search = match &self.popup {
            Some(Popup::Search { search, .. }) => Some(search.clone()),
            _ => None,
        };
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
                let selector = format!("composer-attachment-{index}");
                let thumbnail = preview(format!("{selector}-thumbnail"), attachment, ICON_SIZE, &theme);
                chip(selector, thumbnail, &attachment.display_name, &theme).py_0().pr_0().child(
                    IconButton::new(format!("composer-attachment-{index}-remove"), IconName::X, "Remove", theme)
                        .on_click(remove),
                )
            })
            .collect();
        let attachments = (!chips.is_empty()).then(|| div().px_2().pt_2().flex().flex_wrap().gap_1().children(chips));
        let editor = div()
            .debug_selector(|| text_selector("composer-text", &self.text(cx)))
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .floating(theme)
            .when(focused, |editor| editor.border_color(theme.colors.border))
            .children(attachments)
            .child(self.render_input(cx))
            .child(self.render_actions(cx));
        div()
            .debug_selector(|| "composer".to_owned())
            .mt_2()
            .flex()
            .flex_col()
            .gap_2()
            .children(queued)
            .children(search)
            .children(completion)
            .child(editor)
    }
}

fn editor_style(theme: &UiTheme) -> InputEditorStyle {
    InputEditorStyle { background: theme.colors.input, ..theme.editor_style() }
}

fn round_button(id: &'static str, icon: IconName, label: &'static str, filled: bool, theme: &UiTheme) -> Stateful<Div> {
    let colors = theme.colors;
    let (background, foreground) =
        if filled { (colors.text, colors.canvas) } else { (colors.surface_hover, colors.text_muted) };
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .role(Role::Button)
        .aria_label(label)
        .flex_none()
        .size(rem(ROW_HEIGHT))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .bg(background)
        .hover(move |button| button.opacity(HOVER_OPACITY))
        .tooltip(Tooltip::text(label, *theme))
        .child(Icon::new(icon, foreground))
}

const HOVER_OPACITY: f32 = 0.85;

enum Popup {
    Completion(CompletionMenu),
    Dismissed { token_start: usize },
    Search { search: Entity<PromptSearch>, _subscription: Subscription },
}

const MIN_ROWS: usize = 1;
const MAX_ROWS: usize = 12;

fn mention_text(file: &PromptAttachment) -> String {
    format!("@{}", file.display_name)
}

fn mention_token(file: &PromptAttachment) -> InlineToken {
    InlineToken::new(file.path.to_string_lossy().into_owned(), mention_text(file))
}

fn insert_mention(
    input: &mut TextareaState,
    range: Range<usize>,
    file: &PromptAttachment,
    window: &mut Window,
    cx: &mut Context<TextareaState>,
) {
    if input.replace_range_with_token(range, mention_token(file), window, cx).is_ok() {
        input.insert(" ", window, cx);
    }
}

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
            content = content.clone().with_token(range, mention_token(mention)).unwrap_or(content);
        }
    }
    content
}
