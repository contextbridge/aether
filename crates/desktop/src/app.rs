use crate::chat::SearchPrompts;
use crate::history::History;
use crate::registry::Registry;
use crate::sidebar::{NewThread, Sidebar};
use crate::switcher::{SwitchTarget, SwitcherEvent, ThreadSwitcher};
use crate::target::{AgentCommandLine, Connector};
use crate::thread_view::{ThreadTab, ThreadView};
use crate::threads::{Attention, ThreadEvent, ThreadId, ThreadManager};
use crate::{menus::Quit, notification, theme::DesktopTheme, window_chrome};
use aether_sessions::SessionStore;
use agent_client_protocol::schema::v2::SessionId;
use clankerdiff_gpui::{
    DEFAULT_FONT_FAMILY,
    ui::prelude::{EmptyState, NoticeTone},
};
use gpui::{App, Context, Entity, FocusHandle, KeyBinding, Subscription, Window, actions, div, prelude::*};
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
    sidebar: Entity<Sidebar>,
    views: HashMap<ThreadId, ThreadSlot>,
    /// The selected thread when it was last focused, so focus moves only when the
    /// selection does.
    focused: Option<ThreadId>,
    /// The session selected when the app last ran, resumed once history shows it still exists.
    restore: Option<SessionId>,
    /// The open thread switcher, with the subscription that acts on what it chooses.
    switcher: Option<(Entity<ThreadSwitcher>, Subscription)>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

/// What the app is built from.
pub struct DesktopConfig {
    /// Connects every thread to its agent.
    pub connector: Arc<dyn Connector>,
    /// Starts the agent process of a new local thread.
    pub agent: AgentCommandLine,
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
        let DesktopConfig { connector, agent, sessions, registry } = config;
        install_theme(&registry, window, cx);
        let window_active = window.is_window_active();
        let threads = cx.new(|cx| {
            let mut threads = ThreadManager::new(connector, agent);
            threads.set_window_active(window_active, cx);
            threads
        });
        let history = cx.new(|cx| {
            let mut history = History::new(sessions);
            history.refresh(cx);
            history
        });
        let sidebar = cx.new(|cx| Sidebar::new(threads.clone(), history.clone(), registry.clone(), cx));
        let subscriptions = vec![
            cx.observe_window_appearance(window, |this, window, cx| {
                install_theme(&this.registry, window, cx);
                cx.notify();
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                let active = window.is_window_active();
                this.threads.update(cx, |threads, cx| threads.set_window_active(active, cx));
                if active {
                    this.history.update(cx, History::refresh);
                }
            }),
            cx.observe_window_bounds(window, |this, window, cx| {
                let size = window.window_bounds().get_bounds().size;
                this.registry.update(cx, |registry, cx| registry.set_window_size(size, cx));
            }),
            cx.observe_in(&threads, window, |this, _, window, cx| this.sync_views(window, cx)),
            cx.observe(&history, |this, _, cx| this.restore_selection(cx)),
            cx.subscribe(&threads, |this, _, attention: &Attention, cx| this.announce(attention, cx)),
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
        let restore = registry.read(cx).last_selected().cloned();
        Self {
            threads,
            history,
            registry,
            sidebar,
            views: HashMap::new(),
            focused: None,
            restore,
            switcher: None,
            focus_handle,
            _subscriptions: subscriptions,
        }
    }

    /// Binds the app-level keyboard shortcuts.
    pub fn bind_keys(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("secondary-q", Quit, None),
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

    pub fn sidebar(&self) -> &Entity<Sidebar> {
        &self.sidebar
    }

    pub(crate) fn thread_view(&self, id: ThreadId) -> Option<&Entity<ThreadView>> {
        self.views.get(&id).map(|slot| &slot.view)
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
            projects.iter().flat_map(|project| &project.sessions).find(|entry| *entry.session_id == *session_id.0);
        if let Some(cwd) = saved.map(|entry| entry.cwd.clone()) {
            self.threads.update(cx, |threads, cx| threads.resume(session_id, cwd, cx));
        }
    }

    /// Selects the live thread that has the session open. A session no live thread has open,
    /// because its thread was stopped since, selects nothing.
    fn select_session(&mut self, session_id: &SessionId, cx: &mut Context<Self>) {
        self.threads.update(cx, |threads, cx| {
            if let Some(id) = threads.thread_with_session(session_id, cx).map(|thread| thread.read(cx).id()) {
                threads.select(id, cx);
            }
        });
    }

    /// Posts a native notification for a thread that did something while the user was not
    /// watching it, titled as the thread's header is.
    fn announce(&self, attention: &Attention, cx: &App) {
        let thread = self.threads.read(cx).thread(attention.thread, cx);
        if let (Some(thread), Some(view)) = (thread, self.thread_view(attention.thread)) {
            notification::show(thread.read(cx), &attention.event, view.read(cx).title(cx), cx);
        }
    }

    /// Selects the live thread before or after the selected one in the sidebar's order, wrapping
    /// around at either end. Without a live thread selected, it starts from the first thread going
    /// forward and from the last going back.
    fn step_selection(&mut self, step: Step, cx: &mut Context<Self>) {
        let order = self.sidebar.read(cx).live_order(cx);
        let len = order.len();
        if len == 0 {
            return;
        }
        let selected = self.threads.read(cx).selected();
        let position = selected.and_then(|id| order.iter().position(|&listed| listed == id));
        let next = match (position, step) {
            (Some(position), Step::Next) => (position + 1) % len,
            (Some(position), Step::Previous) => (position + len - 1) % len,
            (None, Step::Next) => 0,
            (None, Step::Previous) => len - 1,
        };
        let id = order[next];
        self.threads.update(cx, |threads, cx| threads.select(id, cx));
    }

    fn open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.switcher.is_some() {
            return;
        }
        let entries = self.sidebar.read(cx).switcher_entries(cx);
        let switcher = cx.new(|cx| ThreadSwitcher::new(entries, window, cx));
        let subscription = cx.subscribe_in(&switcher, window, |this, _, event: &SwitcherEvent, window, cx| {
            if let SwitcherEvent::Chose(target) = event {
                this.threads.update(cx, |threads, cx| match target.clone() {
                    SwitchTarget::Live(id) => threads.select(id, cx),
                    SwitchTarget::Saved { session_id, cwd } => drop(threads.resume(session_id, cwd, cx)),
                });
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
        self.threads.read(cx).selected().and_then(|id| self.thread_view(id)).cloned()
    }

    fn show_tab(&self, tab: ThreadTab, window: &mut Window, cx: &mut App) {
        if let Some(view) = self.selected_view(cx) {
            view.update(cx, |view, cx| view.show(tab, window, cx));
        }
    }

    /// Remembers the selected thread's session, for the next run to select again.
    fn remember_selection(&self, cx: &mut App) {
        let threads = self.threads.read(cx);
        let selected = threads.selected().and_then(|id| threads.thread(id, cx));
        let session_id = selected.and_then(|thread| thread.read(cx).session_id().cloned());
        self.registry.update(cx, |registry, cx| registry.set_last_selected(session_id, cx));
    }

    /// Gives every new thread its view, drops the views of stopped ones, and focuses a newly
    /// selected thread.
    fn sync_views(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let threads = self.threads.read(cx).threads().to_vec();
        let views = self.views.len();
        self.views.retain(|id, _| threads.iter().any(|thread| thread.read(cx).id() == *id));
        if self.views.len() < views {
            // A stopped thread's session shows in history again, which may not have read it yet.
            self.history.update(cx, History::refresh);
        }
        for thread in threads {
            let id = thread.read(cx).id();
            if self.views.contains_key(&id) {
                continue;
            }
            let view = cx.new(|cx| ThreadView::new(thread.clone(), self.registry.clone(), window, cx));
            let turns = cx.subscribe(&thread, |this, _, event: &ThreadEvent, cx| {
                if *event == ThreadEvent::TurnFinished {
                    this.history.update(cx, History::refresh);
                }
            });
            // A new thread learns its session only once its agent opens it.
            let changes = cx.observe(&thread, |this, _, cx| this.remember_selection(cx));
            self.views.insert(id, ThreadSlot { view, _subscriptions: [turns, changes] });
        }
        self.remember_selection(cx);
        let selected = self.threads.read(cx).selected();
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
        let theme = DesktopTheme::global(cx).ui();
        let selected = self.threads.read(cx).selected().and_then(|id| self.thread_view(id));
        let main = div().debug_selector(|| "main".to_owned()).flex_1().min_w_0().h_full().child(match selected {
            Some(view) => view.clone().into_any_element(),
            None => EmptyState::new(
                "No thread selected",
                "Select or start a thread to chat with an agent",
                NoticeTone::Neutral,
                theme,
            )
            .into_any_element(),
        });
        let content = div()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &NewThread, _, cx| this.sidebar.update(cx, Sidebar::new_thread)))
            .on_action(cx.listener(|this, _: &SelectNextThread, _, cx| this.step_selection(Step::Next, cx)))
            .on_action(cx.listener(|this, _: &SelectPreviousThread, _, cx| this.step_selection(Step::Previous, cx)))
            .on_action(cx.listener(|this, _: &OpenThreadSwitcher, window, cx| this.open_switcher(window, cx)))
            .on_action(cx.listener(|this, _: &ShowChat, window, cx| this.show_tab(ThreadTab::Chat, window, cx)))
            .on_action(cx.listener(|this, _: &ShowChanges, window, cx| this.show_tab(ThreadTab::Changes, window, cx)))
            .relative()
            .size_full()
            .flex()
            .bg(theme.colors.canvas)
            .text_color(theme.colors.text)
            .font_family(DEFAULT_FONT_FAMILY)
            .child(self.sidebar.clone())
            .child(main)
            .children(self.switcher.as_ref().map(|(switcher, _)| switcher.clone()))
            .into_any_element();

        window_chrome::decorate(content, &theme, window, cx)
    }
}

#[derive(Clone, Copy)]
enum Step {
    Next,
    Previous,
}

/// The key context of gpui-base's text fields.
const INPUT_CONTEXT: &str = "Input";

/// A thread's view, with the subscriptions that refresh history whenever its turn finishes and
/// remember its session once it is selected.
struct ThreadSlot {
    view: Entity<ThreadView>,
    _subscriptions: [Subscription; 2],
}

/// Installs the user's themes, or else the default ones, in the window's appearance.
fn install_theme(registry: &Entity<Registry>, window: &Window, cx: &mut App) {
    let pair = registry.read(cx).theme_override().cloned().unwrap_or_default();
    DesktopTheme::install(pair, window.appearance(), cx);
}
