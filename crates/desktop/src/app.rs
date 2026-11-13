use crate::agents::{Agents, Catalogs};
use crate::clock::Clock;
use crate::file_picker::file_picker;
use crate::history::History;
use crate::platform::{Notification, Platform};
use crate::registry::{Registry, Saved, WindowSize};
use crate::remotes::Fleet;
use crate::settings::settings;
use crate::sidebar::{Sidebar, SwitcherEntry, toggle_button};
use crate::target::{AgentConnector, Transport};
use crate::tasks::spawn_in;
use crate::thread_view::{ThreadTab, ThreadView};
use crate::threads::{Attention, Thread, ThreadEvent, ThreadManager};
use crate::ui::interface_size::DEFAULT_INTERFACE_SIZE;
use crate::ui::tokens::{HEADER_HEIGHT, MODAL_WIDTH, PANEL_INSET, PICKER_LIST_HEIGHT};
use crate::ui::{
    ActiveTheme, IconName, IndicatorLayer, InterfaceSize, PickerItem, SearchPickerHandle, StyledExt, UI_FONT, UiStyles,
    UiTheme, WispMotion, backdrop, empty_state, key_binding, on_glass, picker_closed, search_picker, window_appearance,
};
use aether_sessions::SessionStore;
use agent_client_protocol::v2::schema::SessionId;
use clankerdiff_core::RepoPath;
use clankerdiff_remote::Remotes;
use clankerdiff_rouge::ReviewHandle;
use clankerdiff_rouge_shell::rouge::core::{
    AnyView, Chord, Div, Length, Styled, View, WindowHandle, div, dynamic, view, window_focused, window_handle,
    window_size,
};
use clankerdiff_rouge_shell::rouge::reactive::{Scope, Signal, Stored, effect, memo, signal, spawn, stored, untrack};
use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectNextThread;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectPreviousThread;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenThreadSwitcher;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShowChat;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShowFiles;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenFile;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenSettings;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NewThread;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NewCloudThread;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToggleSidebar;

pub struct DesktopConfig {
    pub transport: Arc<dyn Transport>,
    pub clock: Arc<dyn Clock>,
    pub agents: Agents,
    pub remotes: Arc<dyn Remotes>,
    pub remotes_file: Option<PathBuf>,
    pub sessions: Option<SessionStore>,
    pub registry: Saved,
    pub platform: Rc<dyn Platform>,
}

#[derive(Clone, Copy)]
pub struct DesktopApp {
    threads: ThreadManager,
    history: History,
    registry: Registry,
    agents: Catalogs,
    fleet: Fleet,
    sidebar: Sidebar,
    views: Stored<HashMap<usize, (Scope, ThreadView)>>,
    modal: Signal<Option<Modal>>,
    platform: Stored<Rc<dyn Platform>>,
    restore: Stored<Option<SessionId>>,
    window: WindowHandle,
    scope: Scope,
}

#[derive(Clone)]
enum Modal {
    Switcher(Rc<[SwitcherEntry]>),
    Files(Thread),
    Settings,
}

pub(crate) const NEW_THREAD_KEYS: &str = "secondary-n";
pub(crate) const NEW_CLOUD_THREAD_KEYS: &str = "secondary-alt-n";
pub(crate) const TOGGLE_SIDEBAR_KEYS: &str = "secondary-b";
pub(crate) const SWITCHER_KEYS: &str = "secondary-k";
pub(crate) const NEXT_THREAD_KEYS: &str = "secondary-]";
pub(crate) const PREVIOUS_THREAD_KEYS: &str = "secondary-[";
pub(crate) const SHOW_CHAT_KEYS: &str = "secondary-1";
pub(crate) const SHOW_FILES_KEYS: &str = "secondary-2";
pub(crate) const OPEN_FILE_KEYS: &str = "secondary-p";
pub(crate) const SETTINGS_KEYS: &str = "secondary-,";

impl DesktopApp {
    pub fn new(config: DesktopConfig) -> Self {
        let DesktopConfig { transport, clock, agents, remotes, remotes_file, sessions, registry, platform } = config;
        let registry = Registry::new(registry);
        let preference = memo(move || registry.with(|state| state.theme.clone()));
        let active = ActiveTheme::install(preference.get_untracked().theme(window_appearance()));
        effect(move || active.apply(&preference.get()));
        let motion = WispMotion::install(registry.with_untracked(|state| state.reduced_motion));
        effect(move || motion.set_reduced(registry.with(|state| state.reduced_motion)));
        let size = InterfaceSize::install(DEFAULT_INTERFACE_SIZE);
        if let Some(saved) = registry.with(|state| state.interface_size) {
            size.set(saved);
        }
        effect(move || {
            let committed = size.committed();
            untrack(|| {
                registry
                    .update(|state| state.interface_size = (committed != DEFAULT_INTERFACE_SIZE).then_some(committed));
            });
        });
        let _ = IndicatorLayer::install();
        let agents = Catalogs::new(agents);
        let connector = Arc::new(AgentConnector::new(remotes.clone(), transport));
        let threads = ThreadManager::new(connector, agents, clock.clone());
        let history = History::new(sessions);
        let fleet = Fleet::new(remotes, remotes_file);
        let platform = stored(platform);
        let sidebar = Sidebar::new(threads, history, registry, agents, fleet, clock, platform);
        let app = Self {
            threads,
            history,
            registry,
            agents,
            fleet,
            sidebar,
            views: stored(HashMap::new()),
            modal: signal(None),
            platform,
            restore: stored(registry.with(|state| state.last_selected.clone())),
            window: window_handle(),
            scope: Scope::new(),
        };
        app.observe();
        app
    }

    pub fn threads(&self) -> ThreadManager {
        self.threads
    }

    pub fn history(&self) -> History {
        self.history
    }

    pub fn registry(&self) -> Registry {
        self.registry
    }

    pub fn fleet(&self) -> Fleet {
        self.fleet
    }

    pub fn diff_review(&self, thread: Thread) -> Option<ReviewHandle> {
        let view = self.views.with(|views| views.get(&thread.number()).map(|(_, view)| *view))?;
        view.files().review()
    }

    pub fn view(self) -> impl View {
        let theme = UiTheme::current();
        let collapsed = memo(move || self.registry.with(|state| state.sidebar_collapsed));
        let selected = memo(move || self.threads.selected());
        let root = view! {
            <div test_id="desktop" relative size_full flex bg=theme.canvas text_color=theme.text font=UI_FONT body>
                <dynamic>{(!collapsed.get()).then(|| self.sidebar.view())}</dynamic>
                <div test_id="main" flex_1 min_w=0.0 h_full>
                    <dynamic>{match selected.get() {
                        Some(thread) => untrack(|| self.view_of(thread).view(collapsed)).into_any(),
                        None => no_thread(collapsed.get()).into_any(),
                    }}</dynamic>
                </div>
                <dynamic>{self.modal.get().map(|modal| untrack(|| self.render_modal(modal)))}</dynamic>
            </div>
        };
        InterfaceSize::current().bind(self.bind(root))
    }

    fn bind(self, root: Div) -> Div {
        let root = root
            .bind_key(chord(NEW_THREAD_KEYS), NewThread)
            .bind_key(chord(NEW_CLOUD_THREAD_KEYS), NewCloudThread)
            .bind_key(chord(TOGGLE_SIDEBAR_KEYS), ToggleSidebar)
            .bind_key(chord(SWITCHER_KEYS), OpenThreadSwitcher)
            .bind_key(chord(NEXT_THREAD_KEYS), SelectNextThread)
            .bind_key(chord(PREVIOUS_THREAD_KEYS), SelectPreviousThread)
            .bind_key(chord(SHOW_CHAT_KEYS), ShowChat)
            .bind_key(chord(SHOW_FILES_KEYS), ShowFiles)
            .bind_key(chord(OPEN_FILE_KEYS), OpenFile)
            .bind_key(chord(SETTINGS_KEYS), OpenSettings);
        root.on_action(move |_: &NewThread| self.sidebar.new_thread())
            .on_action(move |_: &NewCloudThread| self.sidebar.new_cloud_thread())
            .on_action(move |_: &ToggleSidebar| self.toggle_sidebar())
            .on_action(move |_: &SelectNextThread| self.step_selection(1))
            .on_action(move |_: &SelectPreviousThread| self.step_selection(-1))
            .on_action(move |_: &OpenThreadSwitcher| self.open_switcher())
            .on_action(move |_: &OpenSettings| self.present(Modal::Settings))
            .on_action(move |_: &ShowChat| self.show_tab(ThreadTab::Chat))
            .on_action(move |_: &ShowFiles| self.show_tab(ThreadTab::Files))
            .on_action(move |_: &OpenFile| self.open_file_picker())
    }

    fn observe(self) {
        let threads = self.threads;
        threads.on_attention(move |attention| self.on_attention(&attention));
        let focused = window_focused();
        let first = Cell::new(true);
        effect(move || {
            let active = focused.get();
            untrack(|| {
                threads.set_window_active(active);
                if active && !first.replace(false) {
                    self.history.refresh();
                    self.agents.reload();
                    self.fleet.reload();
                }
            });
        });
        self.history.refresh();
        let size = window_size();
        effect(move || {
            let size = size.get();
            untrack(|| {
                self.registry
                    .update(|state| state.window_size = Some(WindowSize { width: size.width, height: size.height }));
            });
        });
        let live = memo(move || threads.threads());
        effect(move || {
            let live = live.get();
            untrack(|| self.drop_views(&live));
        });
        let followed = memo(move || {
            threads
                .threads()
                .iter()
                .map(|thread| (thread.project(), thread.target().instance().map(str::to_owned)))
                .collect::<Vec<_>>()
        });
        let launched = stored(BTreeSet::new());
        effect(move || {
            let followed = followed.get();
            untrack(|| self.follow_threads(&followed, launched));
        });
        let selected_session = memo(move || threads.selected().and_then(|thread| thread.session_id()));
        let remembered = Cell::new(false);
        effect(move || {
            let session_id = selected_session.get();
            if remembered.replace(true) || session_id.is_some() {
                untrack(|| self.registry.update(|state| state.last_selected = session_id));
            }
        });
        effect(move || {
            let projects = self.history.projects();
            untrack(|| self.restore_selection(&projects));
        });
    }

    fn view_of(self, thread: Thread) -> ThreadView {
        if let Some((_, view)) = self.views.with(|views| views.get(&thread.number()).copied()) {
            return view;
        }
        let scope = self.scope.child();
        let view = scope.run(|| ThreadView::new(thread, self.registry, self.platform.get()));
        self.views.update(|views| views.insert(thread.number(), (scope, view)));
        view
    }

    fn drop_views(self, live: &[Thread]) {
        let dropped: Vec<Scope> = self.views.update(|views| {
            let gone: Vec<usize> =
                views.keys().copied().filter(|number| live.iter().all(|thread| thread.number() != *number)).collect();
            gone.into_iter().filter_map(|number| views.remove(&number)).map(|(scope, _)| scope).collect()
        });
        if dropped.is_empty() {
            return;
        }
        self.history.refresh();
        spawn(async move {
            for scope in dropped {
                scope.dispose();
            }
        });
    }

    fn follow_threads(self, followed: &[(Option<PathBuf>, Option<String>)], launched: Stored<BTreeSet<String>>) {
        let listed = self.registry.with(|state| state.projects.clone());
        let unlisted: Vec<PathBuf> =
            followed.iter().filter_map(|(project, _)| project.clone()).filter(|root| !listed.contains(root)).collect();
        if !unlisted.is_empty() {
            self.registry.update(|state| unlisted.into_iter().for_each(|root| state.add_project(root)));
        }
        let now: BTreeSet<String> = followed.iter().filter_map(|(_, instance)| instance.clone()).collect();
        let grew = launched.with(|launched| !now.is_subset(launched));
        launched.set(now);
        if grew {
            self.fleet.refresh();
        }
    }

    fn restore_selection(self, projects: &[crate::history::ProjectHistory]) {
        if self.restore.with(Option::is_none) || projects.is_empty() || self.threads.selected().is_some() {
            return;
        }
        let Some(session_id) = self.restore.update(Option::take) else {
            return;
        };
        let saved = projects.iter().flat_map(|project| &project.sessions).find(|session| session.id == session_id);
        if let Some(cwd) = saved.map(|session| session.cwd.clone()) {
            self.threads.resume(session_id, cwd);
        }
    }

    fn select_session(self, session_id: &SessionId) {
        if let Some(thread) = self.threads.thread_with_session(session_id) {
            self.threads.select(thread);
        }
    }

    fn on_attention(self, attention: &Attention) {
        if attention.event == ThreadEvent::TurnFinished {
            self.history.refresh();
        }
        if !attention.watched {
            self.notify(attention);
        }
    }

    fn notify(self, attention: &Attention) {
        let thread = attention.thread;
        let Some(session_id) = thread.session_id() else {
            return;
        };
        let body = match attention.event {
            ThreadEvent::NeedsInput => {
                thread.elicitation().map_or_else(|| NEEDS_INPUT.to_owned(), |(_, elicitation)| elicitation.question())
            }
            _ => TURN_FINISHED.to_owned(),
        };
        let title = self.registry.with(|state| state.title(Some(&session_id), thread.title()));
        let clicked =
            self.platform.with(|platform| platform.notify(Notification { tag: session_id.0.to_string(), title, body }));
        let window = self.window;
        spawn_in(self.scope, async move {
            if clicked.await {
                window.focus();
                self.select_session(&session_id);
            }
        });
    }

    fn step_selection(self, step: isize) {
        let order = self.sidebar.live_order();
        let len = order.len();
        if len == 0 {
            return;
        }
        let selected = self.threads.selected();
        let position = selected.and_then(|selected| order.iter().position(|listed| *listed == selected));
        let from = position.unwrap_or(if step > 0 { len - 1 } else { 0 });
        let next = (from + len).wrapping_add_signed(step) % len;
        self.threads.select(order[next]);
    }

    fn open_switcher(self) {
        if self.modal.with(Option::is_none) {
            self.present(Modal::Switcher(self.sidebar.switcher_entries().into()));
        }
    }

    fn open_file_picker(self) {
        if let Some(thread) = self.threads.selected().filter(|_| self.modal.with(Option::is_none)) {
            self.present(Modal::Files(thread));
        }
    }

    fn present(self, modal: Modal) {
        if self.modal.with(Option::is_none) {
            self.modal.set(Some(modal));
        }
    }

    fn close_modal(self) {
        self.modal.set(None);
    }

    fn show_tab(self, tab: ThreadTab) {
        if let Some(thread) = self.threads.selected() {
            self.view_of(thread).show(tab);
        }
    }

    fn open_file(self, thread: Thread, path: RepoPath) {
        self.threads.select(thread);
        self.view_of(thread).open_file(path, None);
    }

    fn toggle_sidebar(self) {
        self.registry.update(|state| state.sidebar_collapsed = !state.sidebar_collapsed);
    }

    fn render_modal(self, modal: Modal) -> AnyView {
        match modal {
            Modal::Switcher(entries) => {
                let items = entries
                    .iter()
                    .map(|entry| {
                        PickerItem::new(format!("switcher-{}", entry.key.selector()), entry.title.clone())
                            .detail(entry.project.clone())
                    })
                    .collect();
                let handle = SearchPickerHandle::new(items, move |event| {
                    let closed = picker_closed(&event, |index| {
                        if let Some(entry) = entries.get(index) {
                            self.threads.open(&entry.thread);
                        }
                    });
                    if closed {
                        self.close_modal();
                    }
                });
                view! {
                    <backdrop id="switcher-backdrop" align_top on_dismiss={move || self.close_modal()}>
                        {on_glass(|| view! {
                            <search_picker id="switcher" handle searchable="Search threads…" empty="No matching threads"
                                           footer=FOOTER width={Length::design(MODAL_WIDTH)}
                                           height={Length::design(PICKER_LIST_HEIGHT)}/>
                        })}
                    </backdrop>
                }
                .into_any()
            }
            Modal::Files(thread) => file_picker(thread, move |picked: Option<RepoPath>| {
                self.close_modal();
                if let Some(path) = picked {
                    self.open_file(thread, path);
                }
            })
            .into_any(),
            Modal::Settings => settings(self.registry, move || self.close_modal()).into_any(),
        }
    }
}

pub(crate) fn chord(keys: &str) -> Chord {
    key_binding(keys).unwrap_or_else(|error| unreachable!("built-in chord {keys}: {error}"))
}

fn no_thread(collapsed: bool) -> impl View {
    let header = collapsed.then(|| {
        view! { <div top_bar h=HEADER_HEIGHT pt=PANEL_INSET pr=16.0>{toggle_button(false)}</div> }
    });
    view! {
        <div size_full flex col>
            {header}
            <div flex_1 min_h=0.0 flex items_center justify_center>
                <empty_state icon=IconName::MessageSquareText title="No thread selected"
                             detail="Select a thread, or start a new one"/>
            </div>
        </div>
    }
}

const FOOTER: &str = "↑↓ Move    ↵ Open    Esc Close";
const TURN_FINISHED: &str = "The agent finished its turn";
const NEEDS_INPUT: &str = "The agent needs your input";
