//! The thread sidebar: each of the user's projects with its pinned threads first, then its live
//! threads above its recent saved sessions, and its archived threads in a group of their own at
//! the bottom.

mod project_section;
mod status_indicator;
mod thread_row;

pub use status_indicator::StatusIndicator;
pub use thread_row::relative_time;

use crate::OpenThreadSwitcher;
use crate::agents::{AgentProfile, Agents};
use crate::clock::Clock;
use crate::history::{History, HistoryEntry, git_root};
use crate::registry::{Disclosure, Registry, ThreadMeta};
use crate::switcher::{SwitchTarget, SwitcherEntry};
use crate::theme::DesktopTheme;
use crate::threads::{Thread, ThreadId, ThreadManager, ThreadStatus};
use crate::ui::tokens::{BAR_HEIGHT, RADIUS_CONTROL, rem};
use crate::ui::{
    ActionBar, Button, ButtonVariant, EmptyState, IconButton, IconName, MenuItem, NoticeTone, PanelSide,
    ResizablePanel, Typography, TypographyExt, UiTheme, tokens, with_menu,
};
use agent_client_protocol::schema::v2::SessionId;
use chrono::Duration;
use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, EntityId, MouseButton, PathPromptOptions, Rems, Subscription, Window,
    actions, div, prelude::*, rems,
};
use gpui_base::Selectable as _;
use gpui_base::input::{Enter, Escape, Input, InputState};
use project_section::{ArchivedGroup, ProjectSection};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thread_row::ThreadRow;

/// Width of the thread sidebar until the user drags its edge. It stays fixed as the window
/// resizes.
pub const DEFAULT_SIDEBAR_WIDTH: Rems = tokens::rem(272.0);
pub const MIN_SIDEBAR_WIDTH: Rems = tokens::rem(200.0);
pub const MAX_SIDEBAR_WIDTH: Rems = tokens::rem(480.0);
/// The narrowest dragging the sidebar wider squeezes the selected thread to.
pub const MIN_MAIN_WIDTH: Rems = tokens::rem(320.0);

/// How old a saved session may be and still be listed, unless it is pinned. Older ones are found
/// through the thread switcher.
pub const RECENT: Duration = Duration::days(7);

actions!(
    aether_desktop,
    [
        /// Starts a thread in the selected thread's project, or else in the first project.
        NewThread,
    ]
);

/// Lists the user's projects, those added through "Add project" or by opening a thread in them,
/// each with its threads. Projects only history knows about are not listed.
///
/// Which threads are pinned, archived or renamed comes from the [`Registry`]. The listing is
/// rebuilt only when the threads, history or registry change in a way it shows.
pub struct Sidebar {
    threads: Entity<ThreadManager>,
    history: Entity<History>,
    registry: Entity<Registry>,
    agents: Entity<Agents>,
    clock: Arc<dyn Clock>,
    /// How many saved sessions each project lists, for those whose "Show more" was clicked.
    shown: HashMap<PathBuf, usize>,
    /// Projects whose "Archived" group is expanded.
    archive_expanded: HashSet<PathBuf>,
    /// The thread whose title is being edited.
    rename: Option<Rename>,
    sections: Vec<Section>,
    /// What each live thread's row showed when the sections were last built.
    listed: HashMap<EntityId, Listed>,
    thread_subscriptions: Vec<Subscription>,
    _subscriptions: [Subscription; 4],
}

impl Sidebar {
    pub(crate) fn new(
        threads: Entity<ThreadManager>,
        history: Entity<History>,
        registry: Entity<Registry>,
        agents: Entity<Agents>,
        clock: Arc<dyn Clock>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = [
            cx.observe(&threads, |this, threads, cx| {
                let projects: Vec<PathBuf> =
                    threads.read(cx).threads().iter().map(|thread| thread.read(cx).project().to_path_buf()).collect();
                this.registry.update(cx, |registry, cx| {
                    for project in projects {
                        registry.add_project(project, cx);
                    }
                });
                this.watch_threads(cx);
                this.rebuild(cx);
            }),
            cx.observe(&history, |this, _, cx| this.rebuild(cx)),
            cx.observe(&registry, |this, _, cx| this.rebuild(cx)),
            cx.observe(&agents, |_, _, cx| cx.notify()),
        ];
        let mut sidebar = Self {
            threads,
            history,
            registry,
            agents,
            clock,
            shown: HashMap::new(),
            archive_expanded: HashSet::new(),
            rename: None,
            sections: Vec::new(),
            listed: HashMap::new(),
            thread_subscriptions: Vec::new(),
            _subscriptions: subscriptions,
        };
        sidebar.watch_threads(cx);
        sidebar.sections = sidebar.build_sections(cx);
        sidebar
    }

    /// Starts a thread in the selected thread's project, or else in the first project listed.
    pub(crate) fn new_thread(&mut self, cx: &mut Context<Self>) {
        if let Some(root) = self.default_project(cx) {
            self.new_thread_in(root, cx);
        }
    }

    /// The width the user dragged the sidebar to, or else its default width.
    pub(crate) fn width(&self, cx: &App) -> Rems {
        self.registry.read(cx).sidebar_width().unwrap_or(DEFAULT_SIDEBAR_WIDTH)
    }

    /// The title of the listed row tagged `selector`, if a listed row is.
    pub fn row_title(&self, selector: &str) -> Option<String> {
        self.sections
            .iter()
            .flat_map(|section| section.rows.iter().chain(&section.archived))
            .find(|row| row.selector == selector)
            .map(|row| row.title.clone())
    }

    /// Every live thread: those the sidebar lists, in its order, then those in collapsed
    /// projects, in the order they were opened.
    pub(crate) fn live_order(&self, cx: &App) -> Vec<ThreadId> {
        let mut order: Vec<ThreadId> = self
            .sections
            .iter()
            .flat_map(|section| section.rows.iter().chain(&section.archived))
            .filter_map(|row| match &row.kind {
                RowKind::Live(thread) => Some(thread.read(cx).id()),
                RowKind::Saved(_) => None,
            })
            .collect();
        for thread in self.threads.read(cx).threads() {
            let id = thread.read(cx).id();
            if !order.contains(&id) {
                order.push(id);
            }
        }
        order
    }

    /// Every thread, for the thread switcher: the live ones in [their order](Self::live_order),
    /// then every saved session that no live thread has open, including those the sidebar hides.
    pub(crate) fn switcher_entries(&self, cx: &App) -> Vec<SwitcherEntry> {
        let registry = self.registry.read(cx);
        let threads = self.threads.read(cx);
        let live = self.live_order(cx).into_iter().filter_map(|id| threads.thread(id, cx)).map(|thread| {
            let read = thread.read(cx);
            SwitcherEntry {
                title: Row::live(thread, registry, cx).title,
                project: project_name(read.project()),
                target: SwitchTarget::Live(read.id()),
            }
        });
        let live_sessions = live_sessions(threads.threads(), cx);
        let saved = self.history.read(cx).projects().iter().flat_map(|project| {
            project.sessions.iter().filter(|entry| !live_sessions.contains(entry.session_id.as_str())).map(|entry| {
                SwitcherEntry {
                    title: Row::saved(entry.clone(), registry).title,
                    project: project_name(&project.root),
                    target: SwitchTarget::Saved {
                        session_id: SessionId::new(entry.session_id.as_str()),
                        cwd: entry.cwd.clone(),
                    },
                }
            })
        });
        live.chain(saved).collect()
    }

    /// Drags the sidebar to `width`, leaving the selected thread at least [`MIN_MAIN_WIDTH`].
    fn resize(&mut self, width: Rems, window: &Window, cx: &mut Context<Self>) {
        let available = window.viewport_size().width / window.rem_size();
        let widest = rems(available - MIN_MAIN_WIDTH.0).0.clamp(MIN_SIDEBAR_WIDTH.0, MAX_SIDEBAR_WIDTH.0);
        self.set_width(Some(rems(width.0.min(widest))), cx);
    }

    /// Remembers `width`, or forgets the dragged width so the sidebar takes its default again.
    fn set_width(&mut self, width: Option<Rems>, cx: &mut Context<Self>) {
        self.registry.update(cx, |registry, cx| registry.set_sidebar_width(width, cx));
        cx.notify();
    }

    /// Starts a thread in `root` with the agent last chosen from its new-thread menu, or else its
    /// default agent.
    fn new_thread_in(&self, root: PathBuf, cx: &mut Context<Self>) {
        let catalog = self.agents.read(cx).catalog(&root);
        let last = self.registry.read(cx).last_agent(&root).and_then(|name| catalog.get(name));
        let agent = last.unwrap_or_else(|| catalog.default_agent()).clone();
        self.threads.update(cx, |threads, cx| {
            threads.new_thread(root, agent, cx);
        });
    }

    /// Starts a thread in `root` with `agent`, which later new threads there start with too.
    fn new_thread_with(&self, root: PathBuf, agent: AgentProfile, cx: &mut Context<Self>) {
        let name = agent.name.clone();
        self.registry.update(cx, |registry, cx| registry.set_last_agent(root.clone(), name, cx));
        self.threads.update(cx, |threads, cx| {
            threads.new_thread(root, agent, cx);
        });
    }

    /// What a project's `+` offers when it has more than its default agent to choose from, or
    /// files that could not be read: an entry for each agent, then one that opens each such file.
    fn new_thread_menu(&self, root: &Path, cx: &mut Context<Self>) -> Vec<MenuItem> {
        let catalog = self.agents.read(cx).catalog(root);
        if catalog.agents().len() < 2 && catalog.problems().is_empty() {
            return Vec::new();
        }
        let sidebar = cx.entity();
        let agents = catalog.agents().iter().map(|agent| {
            let (sidebar, root, agent) = (sidebar.clone(), root.to_path_buf(), agent.clone());
            MenuItem::new(format!("new-thread-agent-{}", agent.name), agent.name.clone(), move |_, cx| {
                sidebar.update(cx, |sidebar, cx| sidebar.new_thread_with(root.clone(), agent.clone(), cx));
            })
        });
        let problems = catalog.problems().iter().enumerate().map(|(index, problem)| {
            let file = problem.path().to_path_buf();
            MenuItem::new(format!("new-thread-problem-{index}"), problem.to_string(), move |_, cx| {
                cx.open_with_system(&file);
            })
            .icon(IconName::CircleX)
        });
        agents.chain(problems).collect()
    }

    fn default_project(&self, cx: &App) -> Option<PathBuf> {
        let threads = self.threads.read(cx);
        let selected = threads.selected().and_then(|id| threads.thread(id, cx));
        selected
            .map(|thread| thread.read(cx).project().to_path_buf())
            .or_else(|| self.sections.first().map(|section| section.root.clone()))
    }

    fn add_project(&self, root: PathBuf, cx: &mut Context<Self>) {
        self.registry.update(cx, |registry, cx| registry.add_project(root, cx));
    }

    /// Asks for a folder and adds the project it is in.
    fn browse_for_project(&self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add project".into()),
        });
        let registry = self.registry.clone();
        cx.spawn(async move |_, cx| {
            let Some(paths) = picked.await.ok().and_then(Result::ok).flatten() else {
                return;
            };
            registry.update(cx, |registry, cx| {
                for path in paths {
                    registry.add_project(git_root(&path), cx);
                }
            });
        })
        .detach();
    }

    /// Archives the project's saved sessions that are neither pinned nor open in a live thread.
    fn archive_inactive(&self, root: &Path, cx: &mut Context<Self>) {
        let live_sessions = live_sessions(self.threads.read(cx).threads(), cx);
        let registry = self.registry.read(cx);
        let inactive: Vec<SessionId> = self
            .history
            .read(cx)
            .projects()
            .iter()
            .filter(|project| project.root == root)
            .flat_map(|project| &project.sessions)
            .filter(|entry| !live_sessions.contains(entry.session_id.as_str()))
            .map(|entry| SessionId::new(entry.session_id.as_str()))
            .filter(|session_id| !registry.thread(session_id).pinned)
            .collect();
        self.registry.update(cx, |registry, cx| registry.archive_all(inactive, cx));
    }

    fn remove_project(&self, root: &Path, cx: &mut Context<Self>) {
        self.registry.update(cx, |registry, cx| registry.remove_project(root, cx));
    }

    fn set_disclosure(&self, root: PathBuf, disclosure: Disclosure, cx: &mut Context<Self>) {
        self.registry.update(cx, |registry, cx| registry.set_disclosure(root, disclosure, cx));
    }

    fn show_more(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        *self.shown.entry(root).or_insert(SAVED_ROWS) += MORE_SAVED_ROWS;
        self.rebuild(cx);
    }

    fn toggle_archived(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if !self.archive_expanded.remove(&root) {
            self.archive_expanded.insert(root);
        }
        cx.notify();
    }

    /// Edits the thread's title in place of its row's title, starting from `title`.
    fn start_rename(&mut self, session_id: SessionId, title: &str, window: &mut Window, cx: &mut Context<Self>) {
        let style = DesktopTheme::global(cx).editor_style();
        let title = title.to_owned();
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).default_value(title);
            input.set_editor_style(style);
            input
        });
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        self.rename = Some(Rename { session_id, input });
        cx.notify();
    }

    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        if let Some(Rename { session_id, input }) = self.rename.take() {
            let title = input.read(cx).value();
            self.registry.update(cx, |registry, cx| registry.rename(&session_id, &title, cx));
            cx.notify();
        }
    }

    fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        if self.rename.take().is_some() {
            cx.notify();
        }
    }

    /// The registry's projects, then those of live threads it does not list.
    fn roots<'a>(registry: &'a Registry, threads: &'a [Entity<Thread>], cx: &'a App) -> Vec<&'a Path> {
        let mut seen = HashSet::new();
        registry
            .projects()
            .iter()
            .map(PathBuf::as_path)
            .chain(threads.iter().map(|thread| thread.read(cx).project()))
            .filter(|root| seen.insert(*root))
            .collect()
    }

    /// Observes every live thread, to rebuild the sections when what a row shows of it changes.
    fn watch_threads(&mut self, cx: &mut Context<Self>) {
        let threads = self.threads.read(cx).threads().to_vec();
        self.thread_subscriptions = threads
            .iter()
            .map(|thread| {
                cx.observe(thread, |this, thread, cx| {
                    let listed = Listed::of(thread.read(cx));
                    if this.listed.get(&thread.entity_id()) != Some(&listed) {
                        this.rebuild(cx);
                    }
                })
            })
            .collect();
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.sections = self.build_sections(cx);
        cx.notify();
    }

    fn build_sections(&mut self, cx: &App) -> Vec<Section> {
        self.listed = self
            .threads
            .read(cx)
            .threads()
            .iter()
            .map(|thread| (thread.entity_id(), Listed::of(thread.read(cx))))
            .collect();
        let registry = self.registry.read(cx);
        let threads = self.threads.read(cx).threads();
        let live_sessions = live_sessions(threads, cx);
        let mut live: HashMap<&Path, Vec<Row>> = HashMap::new();
        for thread in threads {
            live.entry(thread.read(cx).project()).or_default().push(Row::live(thread, registry, cx));
        }
        let history: HashMap<&Path, &[HistoryEntry]> = self
            .history
            .read(cx)
            .projects()
            .iter()
            .map(|project| (project.root.as_path(), project.sessions.as_slice()))
            .collect();
        let cutoff = self.clock.now() - RECENT;
        Self::roots(registry, threads, cx)
            .into_iter()
            .map(|root| {
                let disclosure = registry.disclosure(root).unwrap_or(Disclosure::Expanded);
                let mut section =
                    Section { root: root.to_path_buf(), disclosure, rows: Vec::new(), hidden: 0, archived: Vec::new() };
                if disclosure == Disclosure::Collapsed {
                    return section;
                }
                // Most saved sessions are old and unpinned, so they are filtered out before
                // anything is cloned for them.
                let saved = history
                    .get(root)
                    .copied()
                    .unwrap_or_default()
                    .iter()
                    .filter(|entry| !live_sessions.contains(entry.session_id.as_str()))
                    .filter(|entry| {
                        entry.created_at.is_some_and(|created_at| created_at >= cutoff)
                            || registry.thread(&SessionId::new(entry.session_id.as_str())).pinned
                    })
                    .map(|entry| Row::saved(entry.clone(), registry));
                let (archived, listed): (Vec<Row>, Vec<Row>) =
                    live.remove(root).unwrap_or_default().into_iter().chain(saved).partition(|row| row.meta.archived);
                let limit = self.shown.get(root).copied().unwrap_or(SAVED_ROWS);
                (section.rows, section.hidden) = visible(listed, limit);
                section.archived = archived;
                section
            })
            .collect()
    }

    /// The projects history knows about that the sidebar does not list, most recent first.
    fn unlisted_projects(&self, cx: &App) -> Vec<PathBuf> {
        let registry = self.registry.read(cx);
        let listed = Self::roots(registry, self.threads.read(cx).threads(), cx);
        self.history
            .read(cx)
            .projects()
            .iter()
            .map(|project| project.root.clone())
            .filter(|root| !listed.contains(&root.as_path()))
            .take(RECENT_PROJECTS)
            .collect()
    }

    fn render_header(has_projects: bool, theme: &UiTheme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_none()
            .h(rem(BAR_HEIGHT))
            .pl_4()
            .pr_2()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .typography(Typography::Label)
                    .text_color(theme.colors.text_muted)
                    .child("Threads"),
            )
            .child(
                IconButton::new("search-threads", IconName::Search, "Search threads", *theme)
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenThreadSwitcher), cx)),
            )
            .child(
                IconButton::new("new-thread", IconName::SquarePen, "New thread", *theme).when(has_projects, |button| {
                    button.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.new_thread(cx)))
                }),
            )
    }

    /// "Add project", which offers the recent projects history knows about and a folder picker.
    fn render_footer(&self, theme: &UiTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = cx.entity();
        let mut items: Vec<MenuItem> = self
            .unlisted_projects(cx)
            .into_iter()
            .map(|root| {
                let sidebar = sidebar.clone();
                let name = project_name(&root);
                MenuItem::new(format!("add-project-recent-{name}"), name, move |_, cx| {
                    sidebar.update(cx, |sidebar, cx| sidebar.add_project(root.clone(), cx));
                })
            })
            .collect();
        let browse = MenuItem::new("add-project-browse", "Choose folder…", move |_, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.browse_for_project(cx));
        })
        .icon(IconName::FolderPlus);
        items.push(browse);
        let trigger = Button::new("add-project", "Add project", *theme)
            .icon(IconName::Plus)
            .variant(ButtonVariant::Ghost)
            .full_width();
        ActionBar::new(*theme).child(with_menu("add-project", trigger, MouseButton::Left, items, theme).w_full())
    }

    fn render_section(&self, section: &Section, theme: &UiTheme, cx: &mut Context<Self>) -> ProjectSection {
        let Section { root, disclosure, rows, hidden, archived } = section;
        let (disclosure, hidden) = (*disclosure, *hidden);
        let rows: Vec<AnyElement> = rows.iter().map(|row| self.render_row(row, 1, theme, cx)).collect();
        let archived = (!archived.is_empty()).then(|| {
            let toggled = root.clone();
            ArchivedGroup {
                disclosure: if self.archive_expanded.contains(root) {
                    Disclosure::Expanded
                } else {
                    Disclosure::Collapsed
                },
                on_toggle: Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.toggle_archived(toggled.clone(), cx);
                })),
                rows: archived.iter().map(|row| self.render_row(row, 2, theme, cx)).collect(),
            }
        });
        let more = (hidden > 0).then(|| {
            let root = root.clone();
            (hidden, cx.listener(move |this, _: &ClickEvent, _, cx| this.show_more(root.clone(), cx)))
        });
        let sidebar = cx.entity();
        let (archive_root, remove_root) = (root.clone(), root.clone());
        let actions = vec![
            MenuItem::new("menu-archive-inactive", "Archive inactive threads", {
                let sidebar = sidebar.clone();
                move |_, cx| sidebar.update(cx, |sidebar, cx| sidebar.archive_inactive(&archive_root, cx))
            })
            .icon(IconName::Archive),
            MenuItem::new("menu-remove-project", "Remove from sidebar", move |_, cx| {
                sidebar.update(cx, |sidebar, cx| sidebar.remove_project(&remove_root, cx));
            })
            .icon(IconName::X),
        ];
        let toggled = root.clone();
        let new_thread_root = root.clone();
        let new_thread_menu = self.new_thread_menu(root, cx);
        ProjectSection::new(project_name(root), theme)
            .disclosure(disclosure)
            .on_toggle(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.set_disclosure(toggled.clone(), disclosure.toggled(), cx);
            }))
            .on_new_thread(
                cx.listener(move |this, _: &ClickEvent, _, cx| this.new_thread_in(new_thread_root.clone(), cx)),
            )
            .new_thread_menu(new_thread_menu)
            .actions(actions)
            .when_some(more, |section, (hidden, handler)| section.on_show_more(hidden, handler))
            .when_some(archived, ProjectSection::archived)
            .children(rows)
    }

    /// The row, nested `depth` levels under its project, with a context menu of what can be done
    /// with its thread.
    fn render_row(&self, row: &Row, depth: usize, theme: &UiTheme, cx: &mut Context<Self>) -> AnyElement {
        let Row { selector, kind, session_id, title, meta } = row;
        let editing = self.rename.as_ref().filter(|rename| session_id.as_ref() == Some(&rename.session_id));
        let mut items =
            session_id.clone().map(|session_id| self.thread_items(session_id, title, meta, cx)).unwrap_or_default();
        let mut thread_row = ThreadRow::new(selector.clone(), title.clone(), theme).depth(depth).now(self.clock.now());
        if let Some(session_id) = session_id.clone() {
            let registry = self.registry.clone();
            let archived = meta.archived;
            thread_row = thread_row.on_archive(archived, move |_, _, cx| {
                registry.update(cx, |registry, cx| {
                    if archived {
                        registry.unarchive(&session_id, cx);
                    } else {
                        registry.archive(&session_id, cx);
                    }
                });
            });
        }
        match kind {
            RowKind::Live(thread) => {
                let manager = self.threads.read(cx);
                let thread = thread.read(cx);
                let id = thread.id();
                let status = thread.status();
                let unread = manager.is_unread(id);
                let (select, stop, reconnect, menu_stop) =
                    (self.threads.clone(), self.threads.clone(), self.threads.clone(), self.threads.clone());
                thread_row = thread_row
                    .status(&status)
                    .unread(unread)
                    .started_at(Some(thread.opened_at()))
                    .selected(manager.selected() == Some(id))
                    .on_click(move |_, _, cx| select.update(cx, |threads, cx| threads.select(id, cx)))
                    .on_stop(move |_, _, cx| stop.update(cx, |threads, cx| threads.stop(id, cx)))
                    .when(status.can_reconnect(), |row| {
                        row.on_reconnect(move |_, _, cx| reconnect.update(cx, |threads, cx| threads.reconnect(id, cx)))
                    });
                items.push(
                    MenuItem::new("menu-stop", "Stop", move |_, cx| {
                        menu_stop.update(cx, |threads, cx| threads.stop(id, cx));
                    })
                    .icon(IconName::Square),
                );
            }
            RowKind::Saved(HistoryEntry { session_id, cwd, created_at, .. }) => {
                let threads = self.threads.clone();
                let (session_id, cwd) = (session_id.clone(), cwd.clone());
                thread_row =
                    thread_row.status(&ThreadStatus::Dormant).started_at(*created_at).on_click(move |_, _, cx| {
                        let session_id = SessionId::new(session_id.as_str());
                        threads.update(cx, |threads, cx| threads.resume(session_id, cwd.clone(), cx));
                    });
            }
        }
        if let Some(Rename { input, .. }) = editing {
            let thread_row = thread_row.editor(Self::render_editor(input, theme, cx));
            return div().w_full().child(thread_row).into_any_element();
        }
        with_menu(selector, thread_row, MouseButton::Right, items, theme).w_full().into_any_element()
    }

    /// The menu entries every thread with a session has, whether live or saved.
    fn thread_items(&self, session_id: SessionId, title: &str, meta: &ThreadMeta, cx: &Context<Self>) -> Vec<MenuItem> {
        let sidebar = cx.entity();
        let title = title.to_owned();
        let rename_id = session_id.clone();
        let rename = MenuItem::new("menu-rename", "Rename", move |window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.start_rename(rename_id.clone(), &title, window, cx));
        })
        .icon(IconName::Pencil);
        let pin = if meta.pinned {
            self.registry_item("menu-unpin", "Unpin", session_id.clone(), Registry::unpin).icon(IconName::PinOff)
        } else {
            self.registry_item("menu-pin", "Pin", session_id.clone(), Registry::pin).icon(IconName::Pin)
        };
        let archive = if meta.archived {
            self.registry_item("menu-unarchive", "Unarchive", session_id, Registry::unarchive)
                .icon(IconName::ArchiveRestore)
        } else {
            self.registry_item("menu-archive", "Archive", session_id, Registry::archive).icon(IconName::Archive)
        };
        vec![rename, pin, archive]
    }

    /// A menu entry that makes `change` to the thread's metadata.
    fn registry_item(
        &self,
        selector: &'static str,
        label: &'static str,
        session_id: SessionId,
        change: fn(&mut Registry, &SessionId, &mut Context<Registry>),
    ) -> MenuItem {
        let registry = self.registry.clone();
        MenuItem::new(selector, label, move |_, cx| {
            registry.update(cx, |registry, cx| change(registry, &session_id, cx));
        })
    }

    /// The title editor: Enter keeps the title typed into `input`, Escape keeps the old one.
    fn render_editor(input: &Entity<InputState>, theme: &UiTheme, cx: &mut Context<Self>) -> AnyElement {
        div()
            .debug_selector(|| "rename-input".to_owned())
            .w_full()
            .min_w_0()
            .px_1()
            .rounded(rem(RADIUS_CONTROL))
            .border_1()
            .border_color(theme.colors.accent)
            .bg(theme.colors.canvas)
            .on_action(cx.listener(|this, _: &Enter, _, cx| this.commit_rename(cx)))
            .on_action(cx.listener(|this, _: &Escape, _, cx| this.cancel_rename(cx)))
            .child(Input::new(input))
            .into_any_element()
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = DesktopTheme::global(cx).ui();
        let has_projects = !self.sections.is_empty();
        let sidebar = cx.entity().downgrade();
        let reset = sidebar.clone();
        let sections: Vec<ProjectSection> =
            self.sections.iter().map(|section| self.render_section(section, &theme, cx)).collect();
        let projects = if has_projects {
            div().flex_1().min_h_0().child(
                div()
                    .id("projects")
                    .size_full()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(sections),
            )
        } else {
            div().flex_1().min_h_0().child(EmptyState::new(
                "No projects",
                "Add a project to start a thread in it",
                NoticeTone::Neutral,
                theme,
            ))
        };
        ResizablePanel::new("sidebar", PanelSide::Left, self.width(cx), theme)
            .min_width(MIN_SIDEBAR_WIDTH)
            .max_width(MAX_SIDEBAR_WIDTH)
            .on_resize(move |width, window, cx| {
                let _ = sidebar.update(cx, |sidebar, cx| sidebar.resize(width, window, cx));
            })
            .on_reset(move |_, cx| {
                let _ = reset.update(cx, |sidebar, cx| sidebar.set_width(None, cx));
            })
            .bg(theme.colors.surface)
            .child(Self::render_header(has_projects, &theme, cx))
            .child(projects)
            .child(self.render_footer(&theme, cx))
    }
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// How many unpinned saved sessions a project lists before its "Show more" row.
const SAVED_ROWS: usize = 5;

/// How many more saved sessions each click of "Show more" reveals.
const MORE_SAVED_ROWS: usize = 20;

/// How many unlisted projects "Add project" offers.
const RECENT_PROJECTS: usize = 8;

/// A project as the sidebar lists it. Everything but its header is empty while it is collapsed.
struct Section {
    root: PathBuf,
    disclosure: Disclosure,
    /// The rows it shows above its "Show more" row.
    rows: Vec<Row>,
    /// How many saved sessions are hidden behind "Show more".
    hidden: usize,
    archived: Vec<Row>,
}

/// One thread as the sidebar lists it.
struct Row {
    selector: String,
    kind: RowKind,
    /// `None` for a live thread whose agent has not opened its session yet.
    session_id: Option<SessionId>,
    /// The custom title, or else the one derived from the thread's first prompt.
    title: String,
    meta: ThreadMeta,
}

enum RowKind {
    Live(Entity<Thread>),
    /// A saved session no live thread has open. Clicking it resumes the session in a new thread.
    Saved(HistoryEntry),
}

struct Rename {
    session_id: SessionId,
    input: Entity<InputState>,
}

/// What a live thread's row shows of it, besides what the registry and thread manager tell.
#[derive(PartialEq, Eq)]
struct Listed {
    session_id: Option<SessionId>,
    title: String,
    status: ThreadStatus,
}

impl Listed {
    fn of(thread: &Thread) -> Self {
        Self { session_id: thread.session_id().cloned(), title: thread.title(), status: thread.status() }
    }
}

impl Row {
    fn live(thread: &Entity<Thread>, registry: &Registry, cx: &App) -> Self {
        let read = thread.read(cx);
        let session_id = read.session_id().cloned();
        let meta = session_id.as_ref().map(|id| registry.thread(id).clone()).unwrap_or_default();
        Self {
            selector: format!("thread-row-{}", read.id()),
            title: meta.title_or(read.title()),
            kind: RowKind::Live(thread.clone()),
            session_id,
            meta,
        }
    }

    fn saved(entry: HistoryEntry, registry: &Registry) -> Self {
        let session_id = SessionId::new(entry.session_id.as_str());
        let meta = registry.thread(&session_id).clone();
        Self {
            selector: format!("history-row-{}", entry.session_id),
            title: meta.title_or(entry.title.clone()),
            kind: RowKind::Saved(entry),
            session_id: Some(session_id),
            meta,
        }
    }
}

/// The pinned rows first, then the others in their order, of which only the first `limit`
/// unpinned saved sessions are kept; and how many were left out.
fn visible(rows: Vec<Row>, limit: usize) -> (Vec<Row>, usize) {
    let (mut visible, unpinned): (Vec<Row>, Vec<Row>) = rows.into_iter().partition(|row| row.meta.pinned);
    let mut saved = 0;
    let mut hidden = 0;
    for row in unpinned {
        if matches!(row.kind, RowKind::Saved(_)) {
            saved += 1;
            if saved > limit {
                hidden += 1;
                continue;
            }
        }
        visible.push(row);
    }
    (visible, hidden)
}

/// The sessions live threads have open.
fn live_sessions<'a>(threads: &'a [Entity<Thread>], cx: &'a App) -> HashSet<&'a str> {
    threads.iter().filter_map(|thread| thread.read(cx).session_id()).map(|id| &*id.0).collect()
}

fn project_name(root: &Path) -> String {
    root.file_name().map_or_else(|| root.display().to_string(), |name| name.to_string_lossy().into_owned())
}
