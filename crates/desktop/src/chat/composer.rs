use super::completion::{ActiveToken, CompletionItem, CompletionMenu, Trigger};
use super::footer::{config_selector, context_meter, queued_prompts};
use super::prompt_search::{PromptSearch, PromptSearchOutcome};
use crate::file_index::FileIndex;
use crate::theme::DesktopTheme;
use crate::threads::{Thread, ThreadStatus};
use clankerdiff_gpui::ui::prelude::{Button, ButtonVariant};
use client_core::composer::attachment::PromptAttachment;
use client_core::composer::prompt_history::PromptHistory;
use client_core::composer::submission::Submission;
use client_core::session_core::SessionCore;
use gpui::{
    App, ClickEvent, Context, Div, Entity, Focusable, MouseButton, SharedString, Stateful, Subscription, Window,
    actions, div, prelude::*,
};
use gpui_base::input::{
    Enter, Escape, IndentInline, InlineToken, InputContent, InputEvent, MoveDown, MoveUp, Textarea, TextareaState,
};
use std::path::PathBuf;

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
    _subscriptions: Vec<Subscription>,
}

impl Composer {
    pub(crate) fn new(thread: Entity<Thread>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let style = DesktopTheme::global(cx).editor_style();
        let input = cx.new(|cx| {
            let mut input = TextareaState::new(window, cx)
                .auto_grow(MIN_ROWS, MAX_ROWS)
                .submit_on_enter(true)
                .placeholder("Message the agent…  / for commands, @ for files");
            input.set_editor_style(style);
            input
        });
        let subscriptions = vec![
            cx.observe_in(&thread, window, |this, _, window, cx| this.restore_submission(window, cx)),
            cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.history.reset();
                    this.recalling = false;
                    this.refresh_completion(cx);
                }
            }),
            cx.observe_global::<FileIndex>(|this, cx| {
                if this.completion.as_ref().is_some_and(|menu| menu.token().trigger == Trigger::File) {
                    this.refresh_completion(cx);
                }
            }),
            cx.observe_global::<DesktopTheme>(|this, cx| {
                let style = DesktopTheme::global(cx).editor_style();
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
            _subscriptions: subscriptions,
        }
    }

    /// The prompt being written.
    pub(crate) fn text(&self, cx: &App) -> SharedString {
        self.input.read(cx).value()
    }

    pub(crate) fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
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
        if text.trim().is_empty() {
            return;
        }
        let submission = Submission { text, mentions: mentions(&content), media: Vec::new() };
        self.history.push(&submission.text);
        self.history.reset();
        self.recalling = false;
        self.completion = None;
        self.input.update(cx, |input, cx| input.clean(window, cx));
        self.thread.update(cx, |thread, cx| thread.send(submission, cx));
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
                let commands: Vec<_> = thread
                    .core()
                    .map(SessionCore::available_commands)
                    .unwrap_or_default()
                    .iter()
                    // The builtins are wisp's own: the app has its own controls for what they do.
                    .filter(|command| !command.builtin)
                    .cloned()
                    .collect();
                (!commands.is_empty()).then(|| CompletionMenu::commands(token, &text, &commands))
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

    /// Gives back a prompt the agent never received, below anything written since.
    fn restore_submission(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(submission) = self.thread.update(cx, |thread, _| thread.take_restored_submission()) else {
            return;
        };
        let draft = self.input.read(cx).content();
        let content = restored_content(&draft, &submission);
        self.input.update(cx, |input, cx| input.set_value(content, window, cx));
    }

    fn render_input(&self, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = DesktopTheme::global(cx).ui();
        let focused = self.input.focus_handle(cx).is_focused(window);
        div()
            .id("composer-input")
            .flex_1()
            .min_w_0()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(if focused { theme.colors.accent } else { theme.colors.border })
            .bg(theme.colors.surface)
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
            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                if this.completion.is_some() {
                    this.dismiss_completion(cx);
                    cx.stop_propagation();
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

    fn render_footer(&self, cx: &mut Context<Self>) -> Option<Div> {
        let theme = DesktopTheme::global(cx).ui();
        let core = self.thread.read(cx).core()?;
        let selectors: Vec<_> = core
            .session()
            .config_options()
            .iter()
            .filter_map(|option| config_selector(option, &self.thread, &theme))
            .collect();
        let meter = core.conversation().context_usage().map(|usage| context_meter(usage, &theme));
        Some(
            div()
                .debug_selector(|| "composer-footer".to_owned())
                .flex()
                .items_center()
                .gap_1()
                .children(selectors)
                .child(div().flex_1())
                .children(meter),
        )
    }
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = DesktopTheme::global(cx).ui();
        let running = matches!(self.thread.read(cx).status(), ThreadStatus::Running | ThreadStatus::NeedsInput);
        let queued = self
            .thread
            .read(cx)
            .core()
            .map(SessionCore::queued_prompts)
            .filter(|prompts| !prompts.is_empty())
            .map(|prompts| queued_prompts(prompts, &theme));
        let stop = running.then(|| {
            Button::new("stop", "Stop", theme)
                .variant(ButtonVariant::Destructive)
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.thread.update(cx, Thread::cancel)))
        });
        let completion = self.completion.as_ref().map(|menu| menu.render(&theme, cx));
        let search = self.search.as_ref().map(|(search, _)| search.clone());
        let input = self.render_input(window, cx);
        let footer = self.render_footer(cx);
        div()
            .debug_selector(|| "composer".to_owned())
            .flex_none()
            .w_full()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .border_t_1()
            .border_color(theme.colors.border)
            .children(queued)
            .children(search)
            .children(completion)
            .child(div().flex().items_end().gap_2().child(input).children(stop))
            .children(footer)
    }
}

const MIN_ROWS: usize = 1;
const MAX_ROWS: usize = 12;

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

/// `draft` with `submission` below it, its mentions shown as mentions again.
fn restored_content(draft: &InputContent, submission: &Submission) -> InputContent {
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
