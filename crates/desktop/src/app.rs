use crate::agents::Agents;
use crate::chat::SearchPrompts;
use crate::clock::Clock;
use crate::history::History;
use crate::menus::APP_NAME;
use crate::notification;
use crate::registry::Registry;
use crate::sidebar::{NewThread, Sidebar};
use crate::switcher::{self, SwitchTarget};
use crate::target::Connector;
use crate::theme::{DesktopTheme, ThemePair};
use crate::thread_view::{ThreadTab, ThreadView};
use crate::threads::{Attention, Thread, ThreadEvent, ThreadManager};
use crate::ui::interface_size::DEFAULT_INTERFACE_SIZE;
use crate::ui::window_chrome;
use crate::ui::{
    EmptyState, InterfaceSize, NoticeTone, SearchPicker, SearchPickerEvent, Typography, TypographyExt, UI_FONT,
};
use aether_sessions::SessionStore;
use agent_client_protocol::schema::v2::SessionId;
use gpui::{
    App, Context, Entity, EntityId, FocusHandle, KeyBinding, MouseDownEvent, StyleRefinement, Subscription, Window,
    actions, div, prelude::*,
};
use gpui_base::TextSelectionLayer;
use std::collections::HashMap;
use std::sync::Arc;

actions!(
    aether_desktop,
    [
        /// Selects the next live thread in the sidebar, wrapping around after the last.
        SelectNextThread,
        /// Selects the previous live thread in the sidebar, wrapping around before the first.
        SelectPreviousThread,
        /// Opens the thread switcher, to find any thread by its title or project.
        OpenThreadSwitcher,
        /// Shows the selected thread's chat.
        ShowChat,
        /// Shows the changes the selected thread's agent made to the working tree.
        ShowChanges,
    ]
);

/// The root view of the main window: a thread sidebar beside the selected thread.
pub struct DesktopApp {
    threads: Entity<ThreadManager>,
    history: Entity<History>,
    registry: Entity<Registry>,
    agents: Entity<Agents>,
    sidebar: Entity<Sidebar>,
    views: HashMap<EntityId, Entity<ThreadView>>,
    /// The selected thread when it was last focused, so focus moves only when the
    /// selection does.
    focused: Option<EntityId>,
    /// The session selected when the app last ran, resumed once history shows it still exists.
    restore: Option<SessionId>,
    switcher: Option<(Entity<SearchPicker>, Subscription)>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

pub struct DesktopConfig {
    /// Connects every thread to its agent.
    pub connector: Arc<dyn Connector>,
    /// Tells how old saved sessions are.
    pub clock: Arc<dyn Clock>,
    /// The agents each project offers to start threads with.
    pub agents: Agents,
    /// Where saved sessions are read from. Without one the app shows no history.
    pub sessions: Option<SessionStore>,
    /// What the app remembers between runs.
    pub registry: Entity<Registry>,
}

impl DesktopApp {
    /// Creates the root view, installs the theme matching the window's appearance, starts
    /// reading history, and resumes the thread selected when the app last ran once history has
    /// been read. Clicking one of the app's notifications brings the window back with the
    /// notification's thread selected.
    pub fn new(config: DesktopConfig, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let DesktopConfig { connector, clock, agents, sessions, registry } = config;
        DesktopTheme::install(ThemePair::default(), window.appearance(), cx);
        if let Some(size) = registry.read(cx).state().interface_size {
            InterfaceSize::set(size, cx);
        }
        let window_active = window.is_window_active();
        let agents = cx.new(|_| agents);
        let threads = cx.new(|cx| {
            let mut threads = ThreadManager::new(connector, agents.clone());
            threads.set_window_active(window_active, cx);
            threads
        });
        let history = cx.new(|cx| {
            let mut history = History::new(sessions);
            history.refresh(cx);
            history
        });
        let sidebar =
            cx.new(|cx| Sidebar::new(threads.clone(), history.clone(), registry.clone(), agents.clone(), clock, cx));
        let subscriptions = vec![
            cx.observe_window_appearance(window, |_, window, cx| {
                DesktopTheme::install(ThemePair::default(), window.appearance(), cx);
                // Cached views re-render only when notified, and none observes the theme.
                window.refresh();
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                let active = window.is_window_active();
                this.threads.update(cx, |threads, cx| threads.set_window_active(active, cx));
                if active {
                    this.history.update(cx, History::refresh);
                    // The user may have edited an `acp.json` file while away.
                    this.agents.update(cx, Agents::reload);
                }
            }),
            cx.observe_window_bounds(window, |this, window, cx| {
                let size = window.window_bounds().get_bounds().size;
                Registry::update(&this.registry, cx, |state| state.window_size = Some(size));
            }),
            cx.observe_in(&threads, window, |this, _, window, cx| this.sync_views(window, cx)),
            cx.observe_global::<InterfaceSize>(|this, cx| this.remember_interface_size(cx)),
            // The sidebar is laid out at its width before it renders, so its width is the app's to set.
            cx.observe(&sidebar, |_, _, cx| cx.notify()),
            cx.observe(&history, |this, _, cx| this.restore_selection(cx)),
            cx.subscribe(&threads, |this, _, attention: &Attention, cx| this.on_attention(attention, cx)),
        ];
        let app = cx.weak_entity();
        let window_handle = window.window_handle();
        cx.on_system_notification_response(move |response, cx| {
            let session_id = SessionId::new(response.tag.as_ref());
            let _ = window_handle.update(cx, |_, window, cx| {
                window.activate_window();
                app.update(cx, |app, cx| app.select_session(&session_id, cx))
            });
        });
        let focus_handle = cx.focus_handle();
        // Focused so that app shortcuts reach the root before any thread view takes focus.
        focus_handle.focus(window, cx);
        let restore = registry.read(cx).state().last_selected.clone();
        Self {
            threads,
            history,
            registry,
            agents,
            sidebar,
            views: HashMap::new(),
            focused: None,
            restore,
            switcher: None,
            focus_handle,
            _subscriptions: subscriptions,
        }
    }

    pub fn bind_keys(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("secondary-n", NewThread, None),
            KeyBinding::new("secondary-k", OpenThreadSwitcher, None),
            KeyBinding::new("secondary-]", SelectNextThread, None),
            KeyBinding::new("secondary-[", SelectPreviousThread, None),
            KeyBinding::new("secondary-1", ShowChat, None),
            KeyBinding::new("secondary-2", ShowChanges, None),
            // Text fields bind these keys to indenting, and are focused most of the time. Bound
            // later in the same context, these take precedence.
            KeyBinding::new("secondary-]", SelectNextThread, Some(INPUT_CONTEXT)),
            KeyBinding::new("secondary-[", SelectPreviousThread, Some(INPUT_CONTEXT)),
            KeyBinding::new("secondary-r", SearchPrompts, Some(INPUT_CONTEXT)),
        ]);
        SearchPicker::bind_keys(cx);
    }

    pub fn threads(&self) -> &Entity<ThreadManager> {
        &self.threads
    }

    pub fn history(&self) -> &Entity<History> {
        &self.history
    }

    pub fn registry(&self) -> &Entity<Registry> {
        &self.registry
    }

    pub fn agents(&self) -> &Entity<Agents> {
        &self.agents
    }

    pub fn sidebar(&self) -> &Entity<Sidebar> {
        &self.sidebar
    }

    pub(crate) fn thread_view(&self, thread: &Entity<Thread>) -> Option<&Entity<ThreadView>> {
        self.views.get(&thread.entity_id())
    }

    /// Resumes and selects the session selected when the app last ran, unless history no longer
    /// has it or the user has selected a thread since.
    fn restore_selection(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.restore.take() else {
            return;
        };
        if self.threads.read(cx).selected().is_some() {
            return;
        }
        let projects = self.history.read(cx).projects();
        let saved =
            projects.iter().flat_map(|project| &project.sessions).find(|entry| *entry.meta.session_id == *session_id.0);
        if let Some(cwd) = saved.map(|entry| entry.meta.cwd.clone()) {
            self.threads.update(cx, |threads, cx| threads.resume(session_id, cwd, cx));
        }
    }

    /// Selects the live thread that has the session open. A session no live thread has open,
    /// because its thread was stopped since, selects nothing.
    fn select_session(&mut self, session_id: &SessionId, cx: &mut Context<Self>) {
        self.threads.update(cx, |threads, cx| {
            if let Some(thread) = threads.thread_with_session(session_id, cx).cloned() {
                threads.select(&thread, cx);
            }
        });
    }

    /// Refreshes history once a turn finishes, since the agent saved it, and posts a native
    /// notification, titled as the thread's header is, when the user was not watching the thread.
    fn on_attention(&self, attention: &Attention, cx: &mut App) {
        if attention.event == ThreadEvent::TurnFinished {
            self.history.update(cx, History::refresh);
        }
        if let Some(view) = self.thread_view(&attention.thread).filter(|_| !attention.watched) {
            notification::show(attention.thread.read(cx), &attention.event, view.read(cx).title(cx), cx);
        }
    }

    /// Selects the live thread before or after the selected one in the sidebar's order, wrapping
    /// around at either end. Without a live thread selected, it starts from the first thread going
    /// forward and from the last going back.
    fn step_selection(&mut self, step: isize, cx: &mut Context<Self>) {
        let order = self.sidebar.read(cx).live_order(cx);
        let len = order.len();
        if len == 0 {
            return;
        }
        let selected = self.threads.read(cx).selected();
        let position = selected.and_then(|selected| order.iter().position(|listed| listed == selected));
        let from = position.unwrap_or(if step > 0 { len - 1 } else { 0 });
        let next = (from + len).wrapping_add_signed(step) % len;
        self.threads.update(cx, |threads, cx| threads.select(&order[next], cx));
    }

    fn open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.switcher.is_some() {
            return;
        }
        let entries = self.sidebar.read(cx).switcher_entries(cx);
        let switcher = switcher::picker(&entries, window, cx);
        let subscription = cx.subscribe_in(&switcher, window, move |this, _, event: &SearchPickerEvent, window, cx| {
            match event {
                SearchPickerEvent::Chose(index) => {
                    this.threads.update(cx, |threads, cx| match entries[*index].target.clone() {
                        SwitchTarget::Live(thread) => threads.select(&thread, cx),
                        SwitchTarget::Saved { session_id, cwd } => drop(threads.resume(session_id, cwd, cx)),
                    });
                }
                SearchPickerEvent::Dismissed => {}
                SearchPickerEvent::Query(_) => return,
            }
            this.close_switcher(window, cx);
        });
        self.switcher = Some((switcher, subscription));
        cx.notify();
    }

    /// Closes the switcher, giving focus back to the selected thread.
    fn close_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.switcher = None;
        match self.selected_view(cx) {
            Some(view) => view.update(cx, |view, cx| view.focus(window, cx)),
            None => self.focus_handle.focus(window, cx),
        }
        cx.notify();
    }

    fn selected_view(&self, cx: &App) -> Option<Entity<ThreadView>> {
        self.threads.read(cx).selected().and_then(|thread| self.thread_view(thread)).cloned()
    }

    fn show_tab(&self, tab: ThreadTab, window: &mut Window, cx: &mut App) {
        if let Some(view) = self.selected_view(cx) {
            view.update(cx, |view, cx| view.show(tab, window, cx));
        }
    }

    /// Remembers the interface size, forgetting it at the default so the app follows the
    /// platform's default again.
    fn remember_interface_size(&self, cx: &mut App) {
        let size = InterfaceSize::get(cx);
        let chosen = (size != DEFAULT_INTERFACE_SIZE).then_some(size);
        Registry::update(&self.registry, cx, |state| state.interface_size = chosen);
    }

    /// Remembers the selected thread's session, for the next run to select again.
    fn remember_selection(&self, cx: &mut App) {
        let selected = self.threads.read(cx).selected();
        let session_id = selected.and_then(|thread| thread.read(cx).session_id().cloned());
        Registry::update(&self.registry, cx, |state| state.last_selected = session_id);
    }

    /// Gives every new thread its view, drops the views of stopped ones, remembers the selected
    /// thread's session and focuses a newly selected thread. A new thread learns its session only
    /// once its agent opens it, which the manager announces too.
    fn sync_views(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let threads = self.threads.read(cx).threads().to_vec();
        let views = self.views.len();
        self.views.retain(|id, _| threads.iter().any(|thread| thread.entity_id() == *id));
        if self.views.len() < views {
            // A stopped thread's session shows in history again, which may not have read it yet.
            self.history.update(cx, History::refresh);
        }
        for thread in threads {
            if !self.views.contains_key(&thread.entity_id()) {
                let view = cx.new(|cx| ThreadView::new(thread.clone(), self.registry.clone(), window, cx));
                self.views.insert(thread.entity_id(), view);
            }
        }
        self.remember_selection(cx);
        let selected = self.threads.read(cx).selected().map(Entity::entity_id);
        if selected != self.focused {
            self.focused = selected;
            if let Some(view) = self.selected_view(cx) {
                view.update(cx, |view, cx| view.focus(window, cx));
            }
        }
        cx.notify();
    }
}

impl Render for DesktopApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        InterfaceSize::apply(window, cx);
        let theme = DesktopTheme::global(cx).ui();
        let sidebar_width = self.sidebar.read(cx).width(cx);
        let selected = self.threads.read(cx).selected().and_then(|thread| self.thread_view(thread));
        let main = div().debug_selector(|| "main".to_owned()).flex_1().min_w_0().h_full().child(match selected {
            Some(view) => view.clone().cached(StyleRefinement::default().size_full()).into_any_element(),
            None => {
                EmptyState::new("No thread selected", "Select a thread, or start a new one", NoticeTone::Neutral, theme)
                    .into_any_element()
            }
        });
        let content = div()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &NewThread, _, cx| this.sidebar.update(cx, Sidebar::new_thread)))
            .on_action(cx.listener(|this, _: &SelectNextThread, _, cx| this.step_selection(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPreviousThread, _, cx| this.step_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &OpenThreadSwitcher, window, cx| this.open_switcher(window, cx)))
            .on_action(cx.listener(|this, _: &ShowChat, window, cx| this.show_tab(ThreadTab::Chat, window, cx)))
            .on_action(cx.listener(|this, _: &ShowChanges, window, cx| this.show_tab(ThreadTab::Changes, window, cx)))
            .relative()
            .size_full()
            .flex()
            .bg(theme.colors.canvas)
            .text_color(theme.colors.text)
            .font_family(UI_FONT)
            .typography(Typography::Body)
            .child(TextSelectionLayer)
            .child(self.sidebar.clone().cached(StyleRefinement::default().flex_none().w(sidebar_width).h_full()))
            .child(main)
            .children(self.switcher.as_ref().map(|(picker, _)| {
                let dismiss = cx.listener(|this, _: &MouseDownEvent, window, cx| this.close_switcher(window, cx));
                switcher::overlay(picker.clone(), cx, dismiss)
            }))
            .into_any_element();

        window_chrome::decorate(content, APP_NAME, &theme, window, cx)
    }
}

/// The key context of gpui-base's text fields.
const INPUT_CONTEXT: &str = "Input";
