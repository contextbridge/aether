//! The thread sidebar: each of the user's projects with its pinned threads first, then its live
//! threads above its recent saved sessions, and its archived threads in a group of their own at
//! the bottom.

mod thread_row;

pub use thread_row::relative_time;

use crate::OpenThreadSwitcher;
use crate::agents::{AgentProfile, Agents};
use crate::clock::Clock;
use crate::history::{self, History, git_root};
use crate::registry::{Registry, State};
use crate::switcher::{SwitchTarget, SwitcherEntry};
use crate::theme::DesktopTheme;
use crate::threads::{Thread, ThreadManager, ThreadStatus};
use crate::ui::tokens::{BAR_HEIGHT, RADIUS_CONTROL, rem};
use crate::ui::{
    ActionBar, Button, ButtonVariant, EmptyState, Icon, IconButton, IconName, ListRow, MenuItem, NoticeTone, PanelSide,
    ResizablePanel, TreeRow, Typography, TypographyExt, UiTheme, tokens, with_menu,
};
use aether_sessions::SessionSummary;
use agent_client_protocol::schema::v2::SessionId;
use chrono::Duration;
use gpui::{
    AnyElement, App, ClickEvent, Context, Div, Entity, MouseButton, PathPromptOptions, Rems, SharedString,
    Subscription, Window, actions, div, prelude::*, rems,
};
use gpui_base::input::{Enter, Escape, Input, InputState};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thread_row::{slot, tagged, thread_row, trailing};

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
            // The manager notifies whenever a thread's row changes too.
            cx.observe(&threads, |this, threads, cx| {
                let projects: Vec<PathBuf> =
                    threads.read(cx).threads().iter().map(|thread| thread.read(cx).project().to_path_buf()).collect();
                Registry::update(&this.registry, cx, |state| {
                    for root in projects {
                        state.add_project(root);
                    }
                });
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
            _subscriptions: subscriptions,
        };
        sidebar.sections = sidebar.build_sections(cx);
        sidebar
    }

    pub(crate) fn new_thread(&mut self, cx: &mut Context<Self>) {
        if let Some(root) = self.default_project(cx) {
            self.new_thread_in(root, cx);
        }
    }

    /// The width the user dragged the sidebar to, or else its default width.
    pub(crate) fn width(&self, cx: &App) -> Rems {
        self.registry.read(cx).state().sidebar_width.map_or(DEFAULT_SIDEBAR_WIDTH, rems)
    }

    pub fn row_title(&self, selector: &str) -> Option<String> {
        self.sections
            .iter()
            .flat_map(|section| section.rows.iter().chain(&section.archived))
            .find(|row| row.selector == selector)
            .map(|row| row.title.clone())
    }

    /// Every live thread: those the sidebar lists, in its order, then those in collapsed
    /// projects, in the order they were opened.
    pub(crate) fn live_order(&self, cx: &App) -> Vec<Entity<Thread>> {
        let mut order: Vec<Entity<Thread>> = self
            .sections
            .iter()
            .flat_map(|section| section.rows.iter().chain(&section.archived))
            .filter_map(|row| match &row.kind {
                RowKind::Live(thread) => Some(thread.clone()),
                RowKind::Saved(_) => None,
            })
            .collect();
        for thread in self.threads.read(cx).threads() {
            if !order.contains(thread) {
                order.push(thread.clone());
            }
        }
        order
    }

    /// Every thread, for the thread switcher: the live ones in [their order](Self::live_order),
    /// then every saved session that no live thread has open, including those the sidebar hides.
    pub(crate) fn switcher_entries(&self, cx: &App) -> Vec<SwitcherEntry> {
        let registry = self.registry.read(cx).state();
        let live = self.live_order(cx).into_iter().map(|thread| SwitcherEntry {
            title: Row::live(&thread, registry, cx).title,
            project: project_name(thread.read(cx).project()),
            target: SwitchTarget::Live(thread),
        });
        let live_sessions = live_sessions(self.threads.read(cx).threads(), cx);
        let saved = self.history.read(cx).projects().iter().flat_map(|project| {
            project.sessions.iter().filter(|entry| !live_sessions.contains(entry.meta.session_id.as_str())).map(
                |entry| SwitcherEntry {
                    title: Row::saved(entry.clone(), registry).title,
                    project: project_name(&project.root),
                    target: SwitchTarget::Saved {
                        session_id: SessionId::new(entry.meta.session_id.as_str()),
                        cwd: entry.meta.cwd.clone(),
                    },
                },
            )
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
        Registry::update(&self.registry, cx, |state| state.sidebar_width = width.map(|width| width.0));
        cx.notify();
    }

    /// Starts a thread in `root` with the agent last chosen from its new-thread menu, or else its
    /// default agent.
    fn new_thread_in(&self, root: PathBuf, cx: &mut Context<Self>) {
        let catalog = self.agents.read(cx).catalog(&root);
        let last = self.registry.read(cx).state().last_agents.get(&root).and_then(|name| catalog.get(name));
        let agent = last.unwrap_or_else(|| catalog.default_agent()).clone();
        self.threads.update(cx, |threads, cx| {
            threads.new_thread(root, agent, cx);
        });
    }

    /// Starts a thread in `root` with `agent`, which later new threads there start with too.
    fn new_thread_with(&self, root: PathBuf, agent: AgentProfile, cx: &mut Context<Self>) {
        let name = agent.name.clone();
        Registry::update(&self.registry, cx, |state| {
            state.last_agents.insert(root.clone(), name);
        });
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
        let selected = self.threads.read(cx).selected();
        selected
            .map(|thread| thread.read(cx).project().to_path_buf())
            .or_else(|| self.sections.first().map(|section| section.root.clone()))
    }

    fn add_project(&self, root: PathBuf, cx: &mut Context<Self>) {
        Registry::update(&self.registry, cx, |state| state.add_project(root));
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
            Registry::update(&registry, cx, |state| {
                for path in paths {
                    state.add_project(git_root(&path));
                }
            });
        })
        .detach();
    }

    /// Archives the project's saved sessions that are neither pinned nor open in a live thread.
    fn archive_inactive(&self, root: &Path, cx: &mut Context<Self>) {
        let live_sessions = live_sessions(self.threads.read(cx).threads(), cx);
        let pinned = &self.registry.read(cx).state().pinned;
        let inactive: Vec<SessionId> = self
            .history
            .read(cx)
            .projects()
            .iter()
            .filter(|project| project.root == root)
            .flat_map(|project| &project.sessions)
            .filter(|entry| !live_sessions.contains(entry.meta.session_id.as_str()))
            .map(|entry| SessionId::new(entry.meta.session_id.as_str()))
            .filter(|session_id| !pinned.contains(session_id))
            .collect();
        Registry::update(&self.registry, cx, |state| state.archived.extend(inactive));
    }

    fn remove_project(&self, root: &Path, cx: &mut Context<Self>) {
        Registry::update(&self.registry, cx, |state| state.projects.retain(|project| project != root));
    }

    fn toggle_collapsed(&self, root: PathBuf, cx: &mut Context<Self>) {
        Registry::update(&self.registry, cx, |state| {
            if !state.collapsed.remove(&root) {
                state.collapsed.insert(root);
            }
        });
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
        let style = DesktopTheme::global(cx).ui().editor_style();
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
            Registry::update(&self.registry, cx, |state| state.rename(session_id, &title));
            cx.notify();
        }
    }

    fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        if self.rename.take().is_some() {
            cx.notify();
        }
    }

    /// The registry's projects, then those of live threads it does not list.
    fn roots<'a>(registry: &'a State, threads: &'a [Entity<Thread>], cx: &'a App) -> Vec<&'a Path> {
        let mut seen = HashSet::new();
        registry
            .projects
            .iter()
            .map(PathBuf::as_path)
            .chain(threads.iter().map(|thread| thread.read(cx).project()))
            .filter(|root| seen.insert(*root))
            .collect()
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.sections = self.build_sections(cx);
        cx.notify();
    }

    fn build_sections(&self, cx: &App) -> Vec<Section> {
        let registry = self.registry.read(cx).state();
        let threads = self.threads.read(cx).threads();
        let live_sessions = live_sessions(threads, cx);
        let mut live: HashMap<&Path, Vec<Row>> = HashMap::new();
        for thread in threads {
            live.entry(thread.read(cx).project()).or_default().push(Row::live(thread, registry, cx));
        }
        let history: HashMap<&Path, &[SessionSummary]> = self
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
                let collapsed = registry.collapsed.contains(root);
                let mut section =
                    Section { root: root.to_path_buf(), collapsed, rows: Vec::new(), hidden: 0, archived: Vec::new() };
                if collapsed {
                    return section;
                }
                // Most saved sessions are old and unpinned, so they are filtered out before
                // anything is cloned for them.
                let saved = history
                    .get(root)
                    .copied()
                    .unwrap_or_default()
                    .iter()
                    .filter(|entry| !live_sessions.contains(entry.meta.session_id.as_str()))
                    .filter(|entry| {
                        history::created_at(entry).is_some_and(|created_at| created_at >= cutoff)
                            || registry.pinned.contains(&SessionId::new(entry.meta.session_id.as_str()))
                    })
                    .map(|entry| Row::saved(entry.clone(), registry));
                let (archived, listed): (Vec<Row>, Vec<Row>) =
                    live.remove(root).unwrap_or_default().into_iter().chain(saved).partition(|row| row.archived);
                let limit = self.shown.get(root).copied().unwrap_or(SAVED_ROWS);
                (section.rows, section.hidden) = visible(listed, limit);
                section.archived = archived;
                section
            })
            .collect()
    }

    /// The projects history knows about that the sidebar does not list, most recent first.
    fn unlisted_projects(&self, cx: &App) -> Vec<PathBuf> {
        let registry = self.registry.read(cx).state();
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

    /// The project's header, whose `⋯` and `+` show while the pointer is over it, above its
    /// threads, a "Show N more" row and its archived threads.
    fn render_section(&self, section: &Section, theme: &UiTheme, cx: &mut Context<Self>) -> Div {
        let Section { root, collapsed, rows, hidden, archived } = section;
        let (name, muted) = (project_name(root), theme.colors.text_muted);
        let selector = format!("project-{name}");
        let new_thread_selector = format!("{selector}-new-thread");
        let new_thread =
            IconButton::new(new_thread_selector.clone(), IconName::Plus, format!("New thread in {name}"), *theme);
        let new_thread_menu = self.new_thread_menu(root, cx);
        let new_thread = if new_thread_menu.is_empty() {
            let root = root.clone();
            new_thread
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.new_thread_in(root.clone(), cx)))
                .into_any_element()
        } else {
            with_menu(&new_thread_selector, new_thread, MouseButton::Left, new_thread_menu, theme).into_any_element()
        };
        let actions_selector = format!("{selector}-actions");
        let actions = with_menu(
            &actions_selector,
            IconButton::new(actions_selector.clone(), IconName::Ellipsis, "Project actions", *theme),
            MouseButton::Left,
            Self::project_actions(root, cx),
            theme,
        );
        let (folder, state) = if *collapsed { (IconName::Folder, "closed") } else { (IconName::FolderOpen, "open") };
        let toggled = root.clone();
        let header = TreeRow::new(SharedString::from(selector.clone()), name, *theme)
            .expanded(!collapsed)
            .leading(tagged(format!("{selector}-folder-{state}"), Icon::new(folder, muted)))
            .label_color(muted)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_collapsed(toggled.clone(), cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .opacity(0.0)
                    .group_hover(ListRow::hover_group(&selector), |actions| actions.opacity(1.0))
                    .child(actions)
                    .child(new_thread),
            );
        let section = div().w_full().flex().flex_col().child(header);
        if *collapsed {
            return section;
        }
        let rows: Vec<AnyElement> = rows.iter().map(|row| self.render_row(row, 1, theme, cx)).collect();
        let show_more = (*hidden > 0).then(|| {
            let root = root.clone();
            TreeRow::new(SharedString::from(format!("{selector}-show-more")), format!("Show {hidden} more"), *theme)
                .depth(1)
                .leading(slot())
                .label_color(muted)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.show_more(root.clone(), cx)))
        });
        let section = section.children(rows).children(show_more);
        if archived.is_empty() {
            return section;
        }
        let expanded = self.archive_expanded.contains(root);
        let toggled = root.clone();
        let archived_header = TreeRow::new(SharedString::from(format!("{selector}-archived")), "Archived", *theme)
            .depth(1)
            .expanded(expanded)
            .leading(tagged(format!("{selector}-archived-icon"), Icon::new(IconName::Archive, muted)))
            .label_color(muted)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_archived(toggled.clone(), cx)))
            .child(div().typography(Typography::Caption).text_color(muted).child(archived.len().to_string()));
        let archived: Vec<AnyElement> =
            if expanded { archived.iter().map(|row| self.render_row(row, 2, theme, cx)).collect() } else { Vec::new() };
        section.child(archived_header).children(archived)
    }

    fn project_actions(root: &Path, cx: &Context<Self>) -> Vec<MenuItem> {
        let sidebar = cx.entity();
        let (archive_root, remove_root) = (root.to_path_buf(), root.to_path_buf());
        vec![
            MenuItem::new("menu-archive-inactive", "Archive inactive threads", {
                let sidebar = sidebar.clone();
                move |_, cx| sidebar.update(cx, |sidebar, cx| sidebar.archive_inactive(&archive_root, cx))
            })
            .icon(IconName::Archive),
            MenuItem::new("menu-remove-project", "Remove from sidebar", move |_, cx| {
                sidebar.update(cx, |sidebar, cx| sidebar.remove_project(&remove_root, cx));
            })
            .icon(IconName::X),
        ]
    }

    /// The row, nested `depth` levels under its project, with a context menu of what can be done
    /// with its thread. Its archive and stop actions show while the pointer is over it.
    fn render_row(&self, row: &Row, depth: usize, theme: &UiTheme, cx: &mut Context<Self>) -> AnyElement {
        let Row { selector, kind, session_id, title, pinned, archived } = row;
        let mut items = session_id
            .clone()
            .map(|session_id| self.thread_items(session_id, title, *pinned, *archived, cx))
            .unwrap_or_default();
        let mut actions: Vec<IconButton> = session_id
            .clone()
            .into_iter()
            .map(|session_id| {
                let toggle = self.registry_toggle(session_id, |state| &mut state.archived);
                let (action, icon, label) = if *archived {
                    ("unarchive", IconName::ArchiveRestore, "Unarchive")
                } else {
                    ("archive", IconName::Archive, "Archive")
                };
                IconButton::new(format!("{selector}-{action}"), icon, label, *theme)
                    .on_click(move |_, _, cx| toggle(cx))
            })
            .collect();
        let mut reconnect = None;
        let (status, unread, started_at, selected, select): (_, _, _, _, Action) = match kind {
            RowKind::Live(thread) => {
                let manager = self.threads.read(cx);
                let status = thread.read(cx).status();
                let on = |change: fn(&mut ThreadManager, &Entity<Thread>, &mut Context<ThreadManager>)| {
                    let (threads, thread) = (self.threads.clone(), thread.clone());
                    move |cx: &mut App| threads.update(cx, |threads, cx| change(threads, &thread, cx))
                };
                let (stop, menu_stop, reconnect_thread) =
                    (on(ThreadManager::stop), on(ThreadManager::stop), on(ThreadManager::reconnect));
                actions.push(
                    IconButton::new(format!("{selector}-stop"), IconName::Square, "Stop thread", *theme)
                        .on_click(move |_, _, cx| stop(cx)),
                );
                items.push(MenuItem::new("menu-stop", "Stop", move |_, cx| menu_stop(cx)).icon(IconName::Square));
                reconnect = status.can_reconnect().then(|| {
                    IconButton::new(format!("{selector}-reconnect"), IconName::RotateCw, "Reconnect", *theme)
                        .on_click(move |_, _, cx| reconnect_thread(cx))
                });
                let (unread, selected) = (manager.is_unread(thread), manager.selected() == Some(thread));
                (status, unread, Some(thread.read(cx).opened_at()), selected, Box::new(on(ThreadManager::select)))
            }
            RowKind::Saved(summary) => {
                let threads = self.threads.clone();
                let (session_id, cwd) = (SessionId::new(summary.meta.session_id.as_str()), summary.meta.cwd.clone());
                let resume = move |cx: &mut App| {
                    threads.update(cx, |threads, cx| threads.resume(session_id.clone(), cwd.clone(), cx));
                };
                (ThreadStatus::Dormant, false, history::created_at(summary), false, Box::new(resume))
            }
        };
        let editing = self.rename.as_ref().filter(|rename| session_id.as_ref() == Some(&rename.session_id));
        let title = if let Some(Rename { input, .. }) = editing {
            Self::render_editor(input, theme, cx)
        } else {
            let title_selector = format!("{selector}-title");
            div()
                .min_w_0()
                .truncate()
                .debug_selector(move || title_selector)
                .when(matches!(status, ThreadStatus::Dormant), |title| title.text_color(theme.colors.text_muted))
                .child(title.clone())
                .into_any_element()
        };
        let age = started_at.map(|started_at| relative_time(started_at, self.clock.now()));
        let trailing = trailing(selector, age, actions, selected, theme).children(reconnect);
        let row = thread_row(selector, title, &status, unread, trailing, theme).depth(depth).selected(selected);
        if editing.is_some() {
            return div().w_full().child(row).into_any_element();
        }
        let row = row.on_click(move |_, _, cx| select(cx));
        with_menu(selector, row, MouseButton::Right, items, theme).w_full().into_any_element()
    }

    /// The menu entries every thread with a session has, whether live or saved.
    fn thread_items(
        &self,
        session_id: SessionId,
        title: &str,
        pinned: bool,
        archived: bool,
        cx: &Context<Self>,
    ) -> Vec<MenuItem> {
        let sidebar = cx.entity();
        let title = title.to_owned();
        let rename_id = session_id.clone();
        let rename = MenuItem::new("menu-rename", "Rename", move |window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.start_rename(rename_id.clone(), &title, window, cx));
        })
        .icon(IconName::Pencil);
        let pin = self.registry_toggle(session_id.clone(), |state| &mut state.pinned);
        let pin = if pinned {
            MenuItem::new("menu-unpin", "Unpin", move |_, cx| pin(cx)).icon(IconName::PinOff)
        } else {
            MenuItem::new("menu-pin", "Pin", move |_, cx| pin(cx)).icon(IconName::Pin)
        };
        let archive = self.registry_toggle(session_id, |state| &mut state.archived);
        let archive = if archived {
            MenuItem::new("menu-unarchive", "Unarchive", move |_, cx| archive(cx)).icon(IconName::ArchiveRestore)
        } else {
            MenuItem::new("menu-archive", "Archive", move |_, cx| archive(cx)).icon(IconName::Archive)
        };
        vec![rename, pin, archive]
    }

    /// Adds the thread to, or removes it from, the set of threads `set` picks out of the
    /// registry's state.
    fn registry_toggle(
        &self,
        session_id: SessionId,
        set: fn(&mut State) -> &mut HashSet<SessionId>,
    ) -> impl Fn(&mut App) + 'static {
        let registry = self.registry.clone();
        move |cx| {
            Registry::update(&registry, cx, |state| {
                let set = set(state);
                if !set.remove(&session_id) {
                    set.insert(session_id.clone());
                }
            });
        }
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
        let sections: Vec<Div> = self.sections.iter().map(|section| self.render_section(section, &theme, cx)).collect();
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

type Action = Box<dyn Fn(&mut App)>;

/// How many unpinned saved sessions a project lists before its "Show more" row.
const SAVED_ROWS: usize = 5;

/// How many more saved sessions each click of "Show more" reveals.
const MORE_SAVED_ROWS: usize = 20;

/// How many unlisted projects "Add project" offers.
const RECENT_PROJECTS: usize = 8;

/// A project as the sidebar lists it. Everything but its header is empty while it is collapsed.
struct Section {
    root: PathBuf,
    collapsed: bool,
    /// The rows it shows above its "Show more" row.
    rows: Vec<Row>,
    /// How many saved sessions are hidden behind "Show more".
    hidden: usize,
    archived: Vec<Row>,
}

struct Row {
    selector: String,
    kind: RowKind,
    /// `None` for a live thread whose agent has not opened its session yet.
    session_id: Option<SessionId>,
    /// The custom title, or else the one derived from the thread's first prompt.
    title: String,
    pinned: bool,
    archived: bool,
}

enum RowKind {
    Live(Entity<Thread>),
    /// A saved session no live thread has open. Clicking it resumes the session in a new thread.
    Saved(SessionSummary),
}

struct Rename {
    session_id: SessionId,
    input: Entity<InputState>,
}

impl Row {
    fn live(thread: &Entity<Thread>, registry: &State, cx: &App) -> Self {
        let read = thread.read(cx);
        let session_id = read.session_id().cloned();
        Self::new(
            format!("thread-row-{}", read.number()),
            RowKind::Live(thread.clone()),
            session_id,
            read.title(),
            registry,
        )
    }

    fn saved(summary: SessionSummary, registry: &State) -> Self {
        let session_id = SessionId::new(summary.meta.session_id.as_str());
        let selector = format!("history-row-{}", summary.meta.session_id);
        let title = history::title(&summary);
        Self::new(selector, RowKind::Saved(summary), Some(session_id), title, registry)
    }

    fn new(selector: String, kind: RowKind, session_id: Option<SessionId>, title: String, registry: &State) -> Self {
        let has = |set: &HashSet<SessionId>| session_id.as_ref().is_some_and(|id| set.contains(id));
        let (pinned, archived) = (has(&registry.pinned), has(&registry.archived));
        Self { selector, kind, title: registry.title(session_id.as_ref(), title), session_id, pinned, archived }
    }
}

/// The pinned rows first, then the others in their order, of which only the first `limit`
/// unpinned saved sessions are kept; and how many were left out.
fn visible(rows: Vec<Row>, limit: usize) -> (Vec<Row>, usize) {
    let (mut visible, unpinned): (Vec<Row>, Vec<Row>) = rows.into_iter().partition(|row| row.pinned);
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

fn live_sessions<'a>(threads: &'a [Entity<Thread>], cx: &'a App) -> HashSet<&'a str> {
    threads.iter().filter_map(|thread| thread.read(cx).session_id()).map(|id| &*id.0).collect()
}

fn project_name(root: &Path) -> String {
    root.file_name().map_or_else(|| root.display().to_string(), |name| name.to_string_lossy().into_owned())
}
