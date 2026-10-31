use crate::agents::Agents;
use crate::chat::SearchPrompts;
use crate::clock::Clock;
use crate::history::History;
use crate::menus::APP_NAME;
use crate::registry::Registry;
use crate::remotes::Fleet;
use crate::settings::{self, Settings};
use crate::sidebar::{self, NewCloudThread, NewThread, Sidebar, ToggleSidebar};
use crate::target::{AgentConnector, Transport};
use crate::theme::{self, DesktopTheme};
use crate::thread_view::{ThreadTab, ThreadView};
use crate::threads::{Attention, Thread, ThreadEvent, ThreadManager};
use crate::ui::interface_size::DEFAULT_INTERFACE_SIZE;
use crate::ui::tokens::{MODAL_WIDTH, PICKER_LIST_HEIGHT, rem};
use crate::ui::window_chrome;
use crate::ui::{
    Backdrop, EmptyState, IndicatorLayer, InterfaceSize, NoticeTone, PickerItem, SearchPicker, SearchPickerEvent,
    StyledExt, Typography, TypographyExt, UI_FONT, UiTheme, picker_closed, place_window_buttons, spin_when_shown,
};
use aether_sessions::SessionStore;
use agent_client_protocol::schema::v2::SessionId;
use clankerdiff_remote::Remotes;
use gpui::{
    AnyElement, App, Context, Entity, EntityId, FocusHandle, KeyBinding, StyleRefinement, Subscription,
    SystemNotification, Window, actions, div, prelude::*,
};
use gpui_base::TextSelectionLayer;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

actions!(
    aether_desktop,
    [SelectNextThread, SelectPreviousThread, OpenThreadSwitcher, ShowChat, ShowFiles, OpenSettings,]
);

pub struct DesktopApp {
    threads: Entity<ThreadManager>,
    history: Entity<History>,
    registry: Entity<Registry>,
    agents: Entity<Agents>,
    fleet: Entity<Fleet>,
    sidebar: Entity<Sidebar>,
    indicators: Entity<IndicatorLayer>,
    views: HashMap<EntityId, Entity<ThreadView>>,
    focused: Option<EntityId>,
    restore: Option<SessionId>,
    launched: BTreeSet<String>,
    modal: Option<(Modal, Subscription)>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

pub struct DesktopConfig {
    pub transport: Arc<dyn Transport>,
    pub clock: Arc<dyn Clock>,
    pub agents: Agents,
    pub remotes: Arc<dyn Remotes>,
    pub remotes_file: Option<PathBuf>,
    pub sessions: Option<SessionStore>,
    pub registry: Entity<Registry>,
}

impl DesktopApp {
    pub fn new(config: DesktopConfig, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let DesktopConfig { transport, clock, agents, remotes, remotes_file, sessions, registry } = config;
        let preference = registry.read(cx).state().theme.clone();
        theme::apply(&preference, window, cx);
        if let Some(size) = registry.read(cx).state().interface_size {
            InterfaceSize::set(size, cx);
        }
        let window_active = window.is_window_active();
        let agents = cx.new(|_| agents);
        let connector = Arc::new(AgentConnector::new(remotes.clone(), transport));
        let threads = cx.new(|cx| {
            let mut threads = ThreadManager::new(connector, agents.clone(), clock.clone());
            threads.set_window_active(window_active, cx);
            threads
        });
        let history = cx.new(|cx| {
            let mut history = History::new(sessions);
            history.refresh(cx);
            history
        });
        let fleet = cx.new(|cx| Fleet::new(remotes, remotes_file, cx));
        let sidebar = cx.new(|cx| {
            Sidebar::new(threads.clone(), history.clone(), registry.clone(), agents.clone(), fleet.clone(), clock, cx)
        });
        let subscriptions = vec![
            spin_when_shown(window),
            cx.observe_window_appearance(window, |this, window, cx| {
                let preference = this.registry.read(cx).state().theme.clone();
                theme::apply(&preference, window, cx);
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                let active = window.is_window_active();
                this.threads.update(cx, |threads, cx| threads.set_window_active(active, cx));
                if active {
                    this.history.update(cx, History::refresh);
                    this.agents.update(cx, Agents::reload);
                    this.fleet.update(cx, Fleet::reload);
                }
            }),
            cx.observe_window_bounds(window, |this, window, cx| {
                let size = window.window_bounds().get_bounds().size;
                Registry::update(&this.registry, cx, |state| state.window_size = Some(size));
            }),
            cx.observe_in(&threads, window, |this, _, window, cx| this.sync_views(window, cx)),
            cx.observe(&threads, |this, _, cx| this.follow_threads(cx)),
            cx.observe_global::<InterfaceSize>(|this, cx| this.remember_interface_size(cx)),
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
        let indicators = cx.new(|cx| IndicatorLayer::new(window, cx));
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);
        let restore = registry.read(cx).state().last_selected.clone();
        Self {
            threads,
            history,
            registry,
            agents,
            fleet,
            sidebar,
            indicators,
            views: HashMap::new(),
            focused: None,
            restore,
            launched: BTreeSet::new(),
            modal: None,
            focus_handle,
            _subscriptions: subscriptions,
        }
    }

    pub fn bind_keys(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("secondary-n", NewThread, None),
            KeyBinding::new("secondary-alt-n", NewCloudThread, None),
            KeyBinding::new("secondary-b", ToggleSidebar, None),
            KeyBinding::new("secondary-k", OpenThreadSwitcher, None),
            KeyBinding::new("secondary-]", SelectNextThread, None),
            KeyBinding::new("secondary-[", SelectPreviousThread, None),
            KeyBinding::new("secondary-1", ShowChat, None),
            KeyBinding::new("secondary-2", ShowFiles, None),
            KeyBinding::new("secondary-,", OpenSettings, None),
            KeyBinding::new("secondary-]", SelectNextThread, Some(INPUT_CONTEXT)),
            KeyBinding::new("secondary-[", SelectPreviousThread, Some(INPUT_CONTEXT)),
            KeyBinding::new("secondary-r", SearchPrompts, Some(INPUT_CONTEXT)),
        ]);
        SearchPicker::bind_keys(cx);
        Settings::bind_keys(cx);
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

    pub fn fleet(&self) -> &Entity<Fleet> {
        &self.fleet
    }

    fn thread_view(&self, thread: &Entity<Thread>) -> Option<&Entity<ThreadView>> {
        self.views.get(&thread.entity_id())
    }

    fn restore_selection(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.restore.take() else {
            return;
        };
        if self.threads.read(cx).selected().is_some() {
            return;
        }
        let projects = self.history.read(cx).projects();
        let saved = projects.iter().flat_map(|project| &project.sessions).find(|session| session.id == session_id);
        if let Some(cwd) = saved.map(|session| session.cwd.clone()) {
            self.threads.update(cx, |threads, cx| threads.resume(session_id, cwd, cx));
        }
    }

    fn select_session(&mut self, session_id: &SessionId, cx: &mut Context<Self>) {
        self.threads.update(cx, |threads, cx| {
            if let Some(thread) = threads.thread_with_session(session_id, cx).cloned() {
                threads.select(&thread, cx);
            }
        });
    }

    fn on_attention(&self, attention: &Attention, cx: &mut App) {
        if attention.event == ThreadEvent::TurnFinished {
            self.history.update(cx, History::refresh);
        }
        if !attention.watched {
            self.notify(attention, cx);
        }
    }

    fn notify(&self, attention: &Attention, cx: &App) {
        let thread = attention.thread.read(cx);
        let Some(session_id) = thread.session_id() else {
            return;
        };
        let body = match attention.event {
            ThreadEvent::NeedsInput => {
                thread.elicitation().map_or_else(|| NEEDS_INPUT.to_owned(), |(_, elicitation)| elicitation.question())
            }
            _ => TURN_FINISHED.to_owned(),
        };
        cx.show_system_notification(SystemNotification {
            tag: session_id.0.clone().into(),
            title: self.registry.read(cx).state().title(Some(session_id), thread.title()).into(),
            body: body.into(),
            actions: Vec::new(),
        });
    }

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
        if self.modal.is_some() {
            return;
        }
        let entries = self.sidebar.read(cx).switcher_entries(cx);
        let items = entries
            .iter()
            .map(|entry| PickerItem::new(entry.key.selector(), entry.title.clone()).detail(entry.project.clone()))
            .collect();
        let theme = DesktopTheme::global(cx).ui();
        let switcher = cx.new(|cx| {
            SearchPicker::new("switcher", items, theme, cx)
                .searchable("Search threads…", window, cx)
                .empty("No matching threads")
                .footer("↑↓ Move    ↵ Open    Esc Close")
                .width(rem(MODAL_WIDTH))
                .height(rem(PICKER_LIST_HEIGHT))
        });
        let subscription = cx.subscribe_in(&switcher, window, move |this, _, event: &SearchPickerEvent, window, cx| {
            let closed = picker_closed(event, |index| {
                this.threads.update(cx, |threads, cx| threads.open(&entries[index].thread, cx));
            });
            if closed {
                this.close_modal(window, cx);
            }
        });
        self.present(Modal::Switcher(switcher), subscription, cx);
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_some() {
            return;
        }
        let settings = cx.new(|cx| Settings::new(self.registry.clone(), cx));
        settings.update(cx, |settings, cx| settings.focus(window, cx));
        let subscription = cx.subscribe_in(&settings, window, |this, _, _: &settings::Closed, window, cx| {
            this.close_modal(window, cx);
        });
        self.present(Modal::Settings(settings), subscription, cx);
    }

    fn present(&mut self, modal: Modal, subscription: Subscription, cx: &mut Context<Self>) {
        self.modal = Some((modal, subscription));
        cx.notify();
    }

    fn close_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.modal = None;
        self.focus_selected(window, cx);
        cx.notify();
    }

    fn render_modal(&self, theme: &UiTheme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let backdrop = |id: &'static str| {
            let app = cx.entity().downgrade();
            Backdrop::new(id, *theme).align_top().on_dismiss(move |window, cx| {
                let _ = app.update(cx, |this, cx| this.close_modal(window, cx));
            })
        };
        Some(match &self.modal.as_ref()?.0 {
            Modal::Switcher(picker) => backdrop("switcher-backdrop").child(picker.clone()).into_any_element(),
            Modal::Settings(settings) => settings.clone().into_any_element(),
        })
    }

    fn focus_selected(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.selected_view(cx) {
            Some(view) => view.update(cx, |view, cx| view.focus(window, cx)),
            None => self.focus_handle.focus(window, cx),
        }
    }

    fn selected_view(&self, cx: &App) -> Option<Entity<ThreadView>> {
        self.threads.read(cx).selected().and_then(|thread| self.thread_view(thread)).cloned()
    }

    fn show_tab(&self, tab: ThreadTab, window: &mut Window, cx: &mut App) {
        if let Some(view) = self.selected_view(cx) {
            view.update(cx, |view, cx| view.show(tab, window, cx));
        }
    }

    fn toggle_sidebar(&self, cx: &mut Context<Self>) {
        Registry::update(&self.registry, cx, |state| state.sidebar_collapsed = !state.sidebar_collapsed);
        cx.notify();
    }

    fn remember_interface_size(&self, cx: &mut App) {
        let size = InterfaceSize::get(cx);
        let chosen = (size != DEFAULT_INTERFACE_SIZE).then_some(size);
        Registry::update(&self.registry, cx, |state| state.interface_size = chosen);
    }

    fn remember_selection(&self, cx: &mut App) {
        let selected = self.threads.read(cx).selected();
        let session_id = selected.and_then(|thread| thread.read(cx).session_id().cloned());
        Registry::update(&self.registry, cx, |state| state.last_selected = session_id);
    }

    fn follow_threads(&mut self, cx: &mut Context<Self>) {
        let threads = self.threads.read(cx);
        let listed = &self.registry.read(cx).state().projects;
        let unlisted: Vec<PathBuf> = threads
            .projects(cx)
            .filter(|root| !listed.iter().any(|listed| listed == root))
            .map(Path::to_path_buf)
            .collect();
        let launched: BTreeSet<String> = threads
            .threads()
            .iter()
            .filter_map(|thread| thread.read(cx).target().instance().map(str::to_owned))
            .collect();
        if !unlisted.is_empty() {
            Registry::update(&self.registry, cx, |state| unlisted.into_iter().for_each(|root| state.add_project(root)));
        }
        if !launched.is_subset(&self.launched) {
            self.fleet.update(cx, Fleet::refresh);
        }
        self.launched = launched;
    }

    fn sync_views(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let threads = self.threads.read(cx).threads().to_vec();
        let views = self.views.len();
        self.views.retain(|id, _| threads.iter().any(|thread| thread.entity_id() == *id));
        if self.views.len() < views {
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
        place_window_buttons(window);
        let theme = DesktopTheme::global(cx).ui();
        let shown = self.sidebar.read(cx);
        let sidebar_panel = (!shown.collapsed(cx)).then(|| {
            let width = shown.width(cx);
            self.sidebar.clone().cached(StyleRefinement::default().flex_none().w(width).h_full())
        });
        let selected = self.threads.read(cx).selected().and_then(|thread| self.thread_view(thread));
        let main = div().debug_selector(|| "main".to_owned()).flex_1().min_w_0().h_full().child(match selected {
            Some(view) => view.clone().cached(StyleRefinement::default().size_full()).into_any_element(),
            None => div()
                .size_full()
                .flex()
                .flex_col()
                .when(sidebar_panel.is_none(), |main| {
                    main.child(div().top_bar(true, window).pr_4().child(sidebar::toggle_button(false, &theme)))
                })
                .child(div().flex_1().min_h_0().child(EmptyState::new(
                    "No thread selected",
                    "Select a thread, or start a new one",
                    NoticeTone::Neutral,
                    theme,
                )))
                .into_any_element(),
        });
        let content = div()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &NewThread, _, cx| this.sidebar.update(cx, Sidebar::new_thread)))
            .on_action(
                cx.listener(|this, _: &NewCloudThread, _, cx| this.sidebar.update(cx, Sidebar::new_cloud_thread)),
            )
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .on_action(cx.listener(|this, _: &SelectNextThread, _, cx| this.step_selection(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPreviousThread, _, cx| this.step_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &OpenThreadSwitcher, window, cx| this.open_switcher(window, cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)))
            .on_action(cx.listener(|this, _: &ShowChat, window, cx| this.show_tab(ThreadTab::Chat, window, cx)))
            .on_action(cx.listener(|this, _: &ShowFiles, window, cx| this.show_tab(ThreadTab::Files, window, cx)))
            .relative()
            .size_full()
            .flex()
            .bg(theme.colors.canvas)
            .text_color(theme.colors.text)
            .font_family(UI_FONT)
            .typography(Typography::Body)
            .child(TextSelectionLayer)
            .children(sidebar_panel)
            .child(main)
            .child(self.indicators.clone())
            .children(self.render_modal(&theme, cx))
            .into_any_element();

        window_chrome::decorate(content, APP_NAME, &theme, window, cx)
    }
}

enum Modal {
    Switcher(Entity<SearchPicker>),
    Settings(Entity<Settings>),
}

const INPUT_CONTEXT: &str = "Input";

const TURN_FINISHED: &str = "The agent finished its turn";
const NEEDS_INPUT: &str = "The agent needs your input";
