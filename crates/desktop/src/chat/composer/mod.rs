mod completion;
mod pasted;
mod queued;

use super::attachment::{chip, preview};
use crate::app::chord;
use crate::platform::{ClipboardMedia, PathPrompt, Platform};
use crate::tasks::spawn_in;
use crate::threads::{Thread, ThreadEvent, ThreadStatus};
use crate::ui::tokens::{COMPOSER_POPUP_HEIGHT, ICON_SIZE, ROW_HEIGHT};
use crate::ui::typography::Typography;
use crate::ui::{
    Hint, Icon, IconName, PickerItem, SearchPickerEvent, SearchPickerHandle, SurfaceExt, UI_FONT, UiStyles, UiTheme,
    icon_button, picker_closed, search_picker,
};
use clankerdiff_rouge_shell::rouge::core::{
    AnyView, Cursor, FocusHandle, Key, KeyEvent, Length, Modifiers, Styled, View, div, dynamic, editable, focus_handle,
    show, text, view,
};
use clankerdiff_rouge_shell::rouge::reactive::{
    Callback, Scope, Signal, Stored, batch, effect, memo, signal, stored, untrack,
};
use client_core::composer::attachment::{AttachedFile, PromptAttachment, attach_missing, classify_paths};
use client_core::composer::completion_token::Trigger;
use client_core::composer::prompt_history::PromptHistory;
use client_core::composer::prompt_search::PromptSearchStatus;
use client_core::composer::submission::Submission;
use client_core::session_core::SessionCore;
use client_core::session_model::WorkspaceAccess;
use completion::{CompletionItem, CompletionMenu, active_token};
use pasted::PastedImages;
use queued::queued_prompts;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SearchPrompts;

#[derive(Clone, Copy)]
pub(crate) struct Composer {
    thread: Thread,
    text: Signal<String>,
    selection: Signal<Range<usize>>,
    mentions: Signal<Vec<PromptAttachment>>,
    media: Signal<Vec<PromptAttachment>>,
    pasted: Stored<PastedImages>,
    history: Stored<PromptHistory>,
    recalled: Stored<bool>,
    popup: Signal<Popup>,
    search_status: Signal<Option<String>>,
    focus: FocusHandle,
    platform: Stored<Rc<dyn Platform>>,
    scope: Scope,
}

#[derive(Clone)]
enum Popup {
    Closed,
    Completion(CompletionMenu),
    Dismissed { token_start: usize },
    Search(SearchPickerHandle),
}

impl Composer {
    pub(crate) fn new(thread: Thread, platform: Rc<dyn Platform>) -> Self {
        let composer = Self {
            thread,
            text: signal(String::new()),
            selection: signal(0..0),
            mentions: signal(Vec::new()),
            media: signal(Vec::new()),
            pasted: stored(PastedImages::default()),
            history: stored(PromptHistory::default()),
            recalled: stored(false),
            popup: signal(Popup::Closed),
            search_status: signal(None),
            focus: focus_handle(),
            platform: stored(platform),
            scope: Scope::new(),
        };
        thread.on_event(move |event| match event {
            ThreadEvent::SubmissionRestored(submission) => composer.restore(&submission),
            ThreadEvent::PromptSearchUpdated => composer.show_prompt_results(),
            _ => {}
        });
        effect(move || {
            composer.text.get();
            untrack(|| {
                if !composer.recalled.update(std::mem::take) {
                    composer.history.update(PromptHistory::reset);
                }
            });
        });
        effect(move || {
            composer.text.get();
            composer.selection.get();
            untrack(|| composer.refresh_completion());
        });
        let listing = memo(move || thread.listed_files());
        effect(move || {
            listing.with(|_| ());
            untrack(|| {
                if composer.completion().is_some_and(|menu| menu.token().trigger == Trigger::File) {
                    composer.refresh_completion();
                }
            });
        });
        composer
    }

    pub(crate) fn focus(self) {
        self.focus.focus();
    }

    pub(crate) fn attach(self, paths: &[PathBuf]) {
        let remote =
            self.thread.with_core(|core| core.session().workspace_access() == WorkspaceAccess::Remote).unwrap_or(false);
        let mut refused = Vec::new();
        for file in classify_paths(paths.iter().cloned(), &self.root().unwrap_or_default()) {
            match file {
                AttachedFile::Media(media) => self.media.update(|attached| attach_missing(attached, [media])),
                AttachedFile::Mention(mention) => self.insert_mention(self.selection.get_untracked(), mention),
                AttachedFile::Unsupported(file) if remote => refused.push(file.display_name),
                AttachedFile::Unsupported(file) => {
                    self.replace(self.selection.get_untracked(), &format!("{} ", file.path.display()));
                }
            }
        }
        if !refused.is_empty() {
            let message = format!("Can't attach folders or binary files to a remote agent: {}", refused.join(", "));
            self.thread.report_error(&message);
        }
    }

    pub(crate) fn view(self) -> impl View {
        let theme = UiTheme::current();
        let thread = self.thread;
        let queued =
            memo(move || thread.with_core(|core| core.queued_prompts().to_vec()).filter(|prompts| !prompts.is_empty()));
        let completion = memo(move || self.completion());
        let search = memo(move || self.popup.with(|popup| matches!(popup, Popup::Search(_))));
        let editor = view! {
            <div w_full flex col gap=4.0 floating
                 border_color={move || if self.focus.is_focused() { theme.border.get() } else { theme.border_subtle.get() }}>
                <dynamic>{self.attachments()}</dynamic>
                {self.input()}
                {self.actions()}
            </div>
        };
        view! {
            <div test_id="composer" mt=8.0 flex col gap=8.0>
                <dynamic>{queued.get().map(|prompts| queued_prompts(&prompts))}</dynamic>
                <dynamic>{search.get().then(|| untrack(|| self.search_view()))}</dynamic>
                <dynamic>{completion.get().map(|menu| untrack(|| self.completion_view(&menu)))}</dynamic>
                {editor}
            </div>
        }
    }

    fn input(self) -> impl View {
        let theme = UiTheme::current();
        let reading = Typography::Reading;
        let field = editable(self.text)
            .test_id("composer-text")
            .multiline()
            .min_rows(MIN_ROWS)
            .max_rows(MAX_ROWS)
            .submit_on_enter(true)
            .selection(self.selection)
            .track_focus(self.focus)
            .autofocus()
            .font(UI_FONT)
            .text_size(reading.size())
            .line_height(reading.line_height() / reading.size())
            .color(theme.text)
            .w_full()
            .on_key_down(move |event| {
                if self.intercept(event) {
                    event.stop_propagation();
                }
            })
            .on_submit(move |_| self.submit());
        let empty = self.text;
        let placeholder = show(
            move || empty.with(String::is_empty),
            move || {
                view! {
                    <div absolute left=12.0 top=8.0 pass_through>
                        <text reading no_wrap color=theme.text_muted>{PLACEHOLDER}</text>
                    </div>
                }
            },
        );
        div()
            .relative()
            .w_full()
            .min_w(0.0)
            .px(12.0)
            .pt(8.0)
            .cursor(Cursor::Text)
            .bind_key(chord(SEARCH_PROMPTS_KEYS), SearchPrompts)
            .on_action(move |_: &SearchPrompts| self.open_search())
            .child(field)
            .child(placeholder)
    }

    fn actions(self) -> impl View {
        let thread = self.thread;
        let empty = memo(move || self.text.with(|text| text.trim().is_empty()) && self.media.with(Vec::is_empty));
        let running = memo(move || running(thread.status()));
        view! {
            <div px=8.0 pb=8.0 flex items_center justify_between>
                <icon_button id="attach-files" icon=IconName::Paperclip label="Attach files"
                             on_click={move || self.pick_files()}/>
                <dynamic>{
                    if running.get() && empty.get() {
                        round_button("stop", IconName::Square, "Stop (Esc)", true, Callback::from(move || self.cancel()))
                    } else {
                        let filled = !empty.get();
                        round_button("send", IconName::ArrowUp, "Send", filled, Callback::from(move || self.send()))
                    }
                }</dynamic>
            </div>
        }
    }

    fn attachments(self) -> Option<AnyView> {
        let media = self.media.get();
        if media.is_empty() {
            return None;
        }
        let chips = media.iter().enumerate().map(|(index, attachment)| {
            let id = format!("composer-attachment-{index}");
            let thumbnail = preview(format!("{id}-thumbnail"), attachment, ICON_SIZE);
            let remove = view! {
                <icon_button id={format!("{id}-remove")} icon=IconName::X label="Remove"
                             on_click={move || self.remove_media(index)}/>
            };
            chip(id, thumbnail, &attachment.display_name).py(0.0).pr(0.0).child(remove)
        });
        Some(view! { <div px=8.0 pt=8.0 flex flex_wrap gap=4.0>{..chips}</div> }.into_any())
    }

    fn completion_view(self, menu: &CompletionMenu) -> AnyView {
        let choose = Callback::from(move |position: usize| self.accept_completion(position));
        menu.render(choose).into_any()
    }

    fn search_view(self) -> Option<AnyView> {
        let Popup::Search(handle) = self.popup.get_untracked() else {
            return None;
        };
        let status = self.search_status;
        Some(
            view! {
                <search_picker id="prompt-search" handle searchable="Search past prompts…" empty="No matching prompts"
                               max_list_height={Length::design(COMPOSER_POPUP_HEIGHT)} status/>
            }
            .into_any(),
        )
    }

    fn intercept(self, event: &KeyEvent) -> bool {
        let plain = event.modifiers == Modifiers::NONE;
        match event.key {
            Key::ArrowUp if plain => self.step_or_recall(-1),
            Key::ArrowDown if plain => self.step_or_recall(1),
            Key::Tab if plain => self.completion().is_some_and(|menu| {
                self.accept_completion(menu.highlighted());
                true
            }),
            Key::Escape => self.escape(),
            Key::Backspace if plain => self.pop_media(),
            _ => chord(PASTE_KEYS).matches(event) && self.paste(),
        }
    }

    fn submit(self) {
        match self.completion().filter(|menu| menu.item(menu.highlighted()).is_some()) {
            Some(menu) => self.accept_completion(menu.highlighted()),
            None => self.send(),
        }
    }

    fn send(self) {
        let text = self.text.get_untracked();
        let media = self.media.get_untracked();
        if text.trim().is_empty() && media.is_empty() {
            return;
        }
        let mentions = mentioned(&text, &self.mentions.get_untracked());
        let submission = Submission { text: text.clone(), mentions, media };
        self.history.update(|history| history.push(&text));
        batch(|| {
            self.close_completion();
            self.text.set(String::new());
            self.selection.set(0..0);
            self.media.set(Vec::new());
            self.mentions.set(Vec::new());
        });
        self.thread.send(submission);
    }

    fn accept_completion(self, position: usize) {
        let Some(menu) = self.completion() else {
            return;
        };
        self.popup.set(Popup::Closed);
        let Some(item) = menu.item(position).cloned() else {
            return;
        };
        let range = menu.token().range.clone();
        match item {
            CompletionItem::File(file) => self.insert_mention(range, file),
            CompletionItem::Command(command) => {
                let mut replacement = format!("/{}", command.name);
                if command.has_input {
                    replacement.push(' ');
                }
                self.replace(range, &replacement);
                if !command.has_input {
                    self.send();
                }
            }
        }
    }

    fn insert_mention(self, range: Range<usize>, file: PromptAttachment) {
        self.replace(range, &format!("{} ", mention_text(&file)));
        self.mentions.update(|mentions| attach_missing(mentions, [file]));
    }

    fn replace(self, range: Range<usize>, replacement: &str) {
        let mut text = self.text.get_untracked();
        let end = floor_boundary(&text, range.end);
        let start = floor_boundary(&text, range.start.min(end));
        text.replace_range(start..end, replacement);
        let caret = start + replacement.len();
        batch(|| {
            self.text.set(text);
            self.selection.set(caret..caret);
        });
    }

    fn paste(self) -> bool {
        let Some(media) = self.platform.with(|platform| platform.clipboard_media()) else {
            return false;
        };
        match media {
            ClipboardMedia::Files(paths) => self.attach(&paths),
            ClipboardMedia::Png(png) => match self.pasted.update(|pasted| pasted.save(&png)) {
                Ok(image) => self.media.update(|media| attach_missing(media, [image])),
                Err(message) => self.thread.report_error(&message),
            },
        }
        true
    }

    fn pick_files(self) {
        let prompt = PathPrompt { title: "Attach".to_owned(), directories: false, multiple: true };
        let picked = self.platform.with(|platform| platform.pick_paths(prompt));
        spawn_in(self.scope, async move {
            if let Some(paths) = picked.await {
                self.attach(&paths);
                self.focus();
            }
        });
    }

    fn refresh_completion(self) {
        let text = self.text.get_untracked();
        let cursor = self.selection.get_untracked().end.min(text.len());
        let spans = mention_spans(&text, &self.mentions.get_untracked());
        let Some(token) = active_token(&text, cursor, &spans) else {
            self.close_completion();
            return;
        };
        let skip = self.popup.with_untracked(|popup| match popup {
            Popup::Dismissed { token_start } => *token_start == token.range.start,
            Popup::Search(_) => true,
            Popup::Closed | Popup::Completion(_) => false,
        });
        if skip {
            return;
        }
        let menu = match token.trigger {
            Trigger::Command => {
                let commands = self.thread.with_core(|core| core.available_commands().to_vec()).unwrap_or_default();
                (!commands.is_empty()).then(|| CompletionMenu::commands(token, &text, &commands))
            }
            Trigger::File => Some(CompletionMenu::files(token, &text, self.thread.listed_files())),
        };
        self.popup.set(menu.map_or(Popup::Closed, Popup::Completion));
    }

    fn root(self) -> Option<PathBuf> {
        self.thread
            .listed_files()
            .map(|files| files.root)
            .ok()
            .or_else(|| self.thread.target().cwd().map(Path::to_path_buf))
    }

    fn cancel(self) {
        self.thread.update_core(SessionCore::cancel);
    }

    fn completion(self) -> Option<CompletionMenu> {
        self.popup.with(|popup| match popup {
            Popup::Completion(menu) => Some(menu.clone()),
            _ => None,
        })
    }

    fn close_completion(self) {
        if self.popup.with_untracked(|popup| matches!(popup, Popup::Completion(_) | Popup::Dismissed { .. })) {
            self.popup.set(Popup::Closed);
        }
    }

    fn escape(self) -> bool {
        if let Some(menu) = self.completion() {
            self.popup.set(Popup::Dismissed { token_start: menu.token().range.start });
            return true;
        }
        if running(self.thread.status()) {
            self.cancel();
            return true;
        }
        false
    }

    fn pop_media(self) -> bool {
        self.text.with_untracked(String::is_empty) && self.media.try_update(Vec::pop).flatten().is_some()
    }

    fn remove_media(self, index: usize) {
        self.media.update(|media| {
            if index < media.len() {
                media.remove(index);
            }
        });
        self.focus();
    }

    fn step_or_recall(self, by: isize) -> bool {
        if let Some(mut menu) = self.completion() {
            menu.step(by);
            self.popup.set(Popup::Completion(menu));
            return true;
        }
        let text = self.text.get_untracked();
        let recalled = if by < 0 {
            if !text.is_empty() && !self.history.with(PromptHistory::is_recalling) {
                return false;
            }
            self.history.update(|history| history.older(&text).map(str::to_owned))
        } else {
            if !self.history.with(PromptHistory::is_recalling) {
                return false;
            }
            self.history.update(PromptHistory::newer)
        };
        let Some(prompt) = recalled else {
            return false;
        };
        self.set_text(prompt);
        true
    }

    fn set_text(self, value: String) {
        let caret = value.len();
        batch(|| {
            self.recalled.set(true);
            self.close_completion();
            self.text.set(value);
            self.selection.set(caret..caret);
        });
    }

    fn open_search(self) {
        if self.popup.with_untracked(|popup| matches!(popup, Popup::Search(_))) || !self.thread.can_search_prompts() {
            return;
        }
        let handle = self.scope.run(|| {
            SearchPickerHandle::new(Vec::new(), move |event| {
                if let SearchPickerEvent::Query(query) = &event {
                    self.search_prompts(query);
                }
                let closed = picker_closed(&event, |index| {
                    let prompt = self.thread.with_core(|core| {
                        core.prompt_search().results().get(index).map(|result| result.prompt.clone())
                    });
                    if let Some(prompt) = prompt.flatten() {
                        self.set_text(prompt);
                    }
                });
                if closed {
                    self.popup.set(Popup::Closed);
                    self.focus();
                }
            })
            .unfiltered()
        });
        self.popup.set(Popup::Search(handle));
        self.search_prompts("");
    }

    fn search_prompts(self, query: &str) {
        self.thread.update_core(|core| core.search_prompts(query));
        self.show_prompt_results();
    }

    fn show_prompt_results(self) {
        let Popup::Search(handle) = self.popup.get_untracked() else {
            return;
        };
        let Some((items, status)) = self.thread.with_core(|core| {
            let search = core.prompt_search();
            let items: Vec<PickerItem> = search
                .results()
                .iter()
                .enumerate()
                .map(|(n, result)| {
                    PickerItem::new(
                        format!("prompt-search-result-{n}"),
                        result.prompt.lines().next().unwrap_or_default().to_owned(),
                    )
                })
                .collect();
            let status = match search.status() {
                PromptSearchStatus::Failed(error) => Some(format!("Search failed: {error}")),
                PromptSearchStatus::Searching => Some("Searching…".to_owned()),
                PromptSearchStatus::Blank => Some("Type to search the prompts you have sent".to_owned()),
                PromptSearchStatus::NoMatches | PromptSearchStatus::Results => None,
            };
            (items, status)
        }) else {
            return;
        };
        handle.set_items(items);
        self.search_status.set_if_changed(status);
    }

    fn restore(self, submission: &Submission) {
        if !submission.text.is_empty() {
            let draft = self.text.get_untracked();
            self.set_text(submission.restore_onto(&draft));
        }
        self.mentions.update(|mentions| attach_missing(mentions, submission.mentions.iter().cloned()));
        self.media.update(|media| attach_missing(media, submission.media.iter().cloned()));
    }
}

fn running(status: ThreadStatus) -> bool {
    matches!(status, ThreadStatus::Running | ThreadStatus::Background | ThreadStatus::NeedsInput)
}

fn round_button(id: &'static str, icon: IconName, label: &'static str, filled: bool, on_click: Callback) -> AnyView {
    let theme = UiTheme::current();
    let (background, foreground) =
        if filled { (theme.text, theme.canvas) } else { (theme.surface_hover, theme.text_muted) };
    let hovered = signal(false);
    view! {
        <div test_id=id flex_none size=ROW_HEIGHT flex items_center justify_center rounded={ROW_HEIGHT / 2.0}
             bg={move || {
                 let color = background.get();
                 if hovered.get() { color.with_alpha(color.a * HOVER_OPACITY) } else { color }
             }}
             cursor=Cursor::Pointer preserve_focus
             on_hover={move |inside| {
                 hovered.set_if_changed(inside);
             }}
             on_click={move |event| {
                 event.stop_propagation();
                 on_click.call();
             }}>
            {Icon::new(icon, foreground)}
        </div>
    }
    .tooltip(label)
    .into_any()
}

fn mention_text(file: &PromptAttachment) -> String {
    format!("@{}", file.display_name)
}

fn mention_spans(text: &str, mentions: &[PromptAttachment]) -> Vec<Range<usize>> {
    mentions
        .iter()
        .flat_map(|mention| {
            let needle = mention_text(mention);
            text.match_indices(&needle).map(|(start, _)| start..start + needle.len()).collect::<Vec<_>>()
        })
        .collect()
}

fn mentioned(text: &str, mentions: &[PromptAttachment]) -> Vec<PromptAttachment> {
    mentions.iter().filter(|mention| text.contains(&mention_text(mention))).cloned().collect()
}

fn floor_boundary(text: &str, index: usize) -> usize {
    text.floor_char_boundary(index.min(text.len()))
}

const MIN_ROWS: usize = 1;
const MAX_ROWS: usize = 12;
const HOVER_OPACITY: f32 = 0.85;
const SEARCH_PROMPTS_KEYS: &str = "secondary-r";
const PASTE_KEYS: &str = "secondary-v";
const PLACEHOLDER: &str = "Message the agent…  / for commands, @ for files";
