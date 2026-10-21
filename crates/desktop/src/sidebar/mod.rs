//! The thread sidebar: each project with its pinned threads first, then its live threads above
//! its saved sessions, and its archived threads in a group of their own at the bottom.

mod project_section;
mod status_dot;
mod thread_menu;
mod thread_row;

pub use status_dot::{Seen, StatusDot};
pub use thread_row::relative_time;

use crate::history::{History, HistoryEntry, git_root};
use crate::registry::{Disclosure, Registry, ThreadMeta};
use crate::switcher::{SwitchTarget, SwitcherEntry};
use crate::theme::DesktopTheme;
use crate::threads::{Thread, ThreadId, ThreadManager};
use agent_client_protocol::schema::v2::SessionId;
use clankerdiff_gpui::ui::prelude::{Button, ButtonVariant, EmptyState, NoticeTone, UiTheme};
use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, PathPromptOptions, Pixels, Subscription, Window, actions, div,
    prelude::*, px,
};
use gpui_base::Selectable as _;
use gpui_base::input::{Enter, Escape, Input, InputState};
use project_section::{ArchivedGroup, ProjectSection};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use thread_menu::MenuItem;
use thread_row::ThreadRow;

/// Width of the thread sidebar, which stays fixed as the window resizes.
pub const SIDEBAR_WIDTH: Pixels = px(260.0);

actions!(
    aether_desktop,
    [
        /// Starts a thread in the selected thread's project, or else in the first project.
        NewThread,
    ]
);

/// Lists the projects, added ones first and then those only history or live threads know
/// about, each with its threads.
///
/// Which threads are pinned, archived or renamed comes from the [`Registry`] each time the
/// sidebar renders.
pub struct Sidebar {
    threads: Entity<ThreadManager>,
    history: Entity<History>,
    registry: Entity<Registry>,
    /// How many saved sessions each project lists, for those whose "Show more" was clicked.
    shown: HashMap<PathBuf, usize>,
    /// Projects a thread was opened in this run.
    opened: HashSet<PathBuf>,
    /// Projects whose "Archived" group is expanded.
    archive_expanded: HashSet<PathBuf>,
    /// The thread whose title is being edited.
    rename: Option<Rename>,
    _threads_changed: Subscription,
}

impl Sidebar {
    pub(crate) fn new(
        threads: Entity<ThreadManager>,
        history: Entity<History>,
        registry: Entity<Registry>,
        cx: &mut Context<Self>,
    ) -> Self {
        let threads_changed = cx.observe(&threads, |this, threads, cx| {
            let projects = threads.read(cx).threads().iter().map(|thread| thread.read(cx).project().to_path_buf());
            this.opened.extend(projects);
        });
        Self {
            threads,
            history,
            registry,
            shown: HashMap::new(),
            opened: HashSet::new(),
            archive_expanded: HashSet::new(),
            rename: None,
            _threads_changed: threads_changed,
        }
    }

    /// Starts a thread in the selected thread's project, or else in the first project listed.
    pub(crate) fn new_thread(&mut self, cx: &mut Context<Self>) {
        if let Some(root) = self.default_project(cx) {
            self.new_thread_in(root, cx);
        }
    }

    /// The title of the listed row tagged `selector`, if a listed row is.
    pub fn row_title(&self, selector: &str, cx: &App) -> Option<String> {
        self.sections(cx)
            .into_iter()
            .flat_map(|section| section.rows.into_iter().chain(section.archived))
            .find(|row| row.selector == selector)
            .map(|row| row.title)
    }

    /// Every live thread: those the sidebar lists, in its order, then those in collapsed
    /// projects, in the order they were opened.
    pub(crate) fn live_order(&self, cx: &App) -> Vec<ThreadId> {
        let mut order: Vec<ThreadId> = self
            .sections(cx)
            .into_iter()
            .flat_map(|section| section.rows.into_iter().chain(section.archived))
            .filter_map(|row| match row.kind {
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
    /// then each project's saved sessions that no live thread has open, including those the
    /// sidebar hides.
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
        let live_sessions: HashSet<&str> =
            threads.threads().iter().filter_map(|thread| thread.read(cx).session_id()).map(|id| &*id.0).collect();
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

    fn new_thread_in(&self, root: PathBuf, cx: &mut Context<Self>) {
        self.threads.update(cx, |threads, cx| {
            threads.new_thread(root, cx);
        });
    }

    fn default_project(&self, cx: &App) -> Option<PathBuf> {
        let threads = self.threads.read(cx);
        let selected = threads.selected().and_then(|id| threads.thread(id, cx));
        selected
            .map(|thread| thread.read(cx).project().to_path_buf())
            .or_else(|| self.sections(cx).into_iter().next().map(|section| section.root))
    }

    /// Asks for a folder and adds the project it is in.
    fn add_project(&self, cx: &mut Context<Self>) {
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

    fn set_disclosure(&self, root: PathBuf, disclosure: Disclosure, cx: &mut Context<Self>) {
        self.registry.update(cx, |registry, cx| registry.set_disclosure(root, disclosure, cx));
    }

    fn show_more(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        *self.shown.entry(root).or_insert(SAVED_ROWS) += MORE_SAVED_ROWS;
        cx.notify();
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

    fn sections(&self, cx: &App) -> Vec<Section> {
        let registry = self.registry.read(cx);
        let threads = self.threads.read(cx).threads();
        let live_sessions: HashSet<&str> =
            threads.iter().filter_map(|thread| thread.read(cx).session_id()).map(|id| &*id.0).collect();
        let mut live: HashMap<&Path, Vec<Row>> = HashMap::new();
        for thread in threads {
            live.entry(thread.read(cx).project()).or_default().push(Row::live(thread, registry, cx));
        }
        let projects = self.history.read(cx).projects();
        let history: HashMap<&Path, &[HistoryEntry]> =
            projects.iter().map(|project| (project.root.as_path(), project.sessions.as_slice())).collect();
        let roots = registry
            .projects()
            .iter()
            .map(PathBuf::as_path)
            .chain(projects.iter().map(|project| project.root.as_path()))
            .chain(threads.iter().map(|thread| thread.read(cx).project()));
        let mut seen = HashSet::new();
        roots
            .filter(|root| seen.insert(*root))
            .map(|root| {
                let disclosure = self.disclosure(root, registry);
                let mut section = Section {
                    root: root.to_path_buf(),
                    disclosure,
                    rows: Vec::new(),
                    more: false,
                    archived: Vec::new(),
                };
                if disclosure == Disclosure::Collapsed {
                    return section;
                }
                let saved = history
                    .get(root)
                    .copied()
                    .unwrap_or_default()
                    .iter()
                    .filter(|entry| !live_sessions.contains(entry.session_id.as_str()))
                    .map(|entry| Row::saved(entry.clone(), registry));
                let (archived, listed): (Vec<Row>, Vec<Row>) =
                    live.remove(root).unwrap_or_default().into_iter().chain(saved).partition(|row| row.meta.archived);
                let limit = self.shown.get(root).copied().unwrap_or(SAVED_ROWS);
                (section.rows, section.more) = visible(listed, limit);
                section.archived = archived;
                section
            })
            .collect()
    }

    /// The user's choice if they made one. Otherwise a project starts expanded when it was added
    /// or a thread was opened in it, and collapsed when only history knows about it.
    fn disclosure(&self, root: &Path, registry: &Registry) -> Disclosure {
        registry.disclosure(root).unwrap_or_else(|| {
            if self.opened.contains(root) || registry.projects().iter().any(|added| added == root) {
                Disclosure::Expanded
            } else {
                Disclosure::Collapsed
            }
        })
    }

    fn render_section(&self, section: Section, theme: &UiTheme, cx: &mut Context<Self>) -> ProjectSection {
        let Section { root, disclosure, rows, more, archived } = section;
        let rows: Vec<AnyElement> = rows.into_iter().map(|row| self.render_row(row, theme, cx)).collect();
        let archived = (!archived.is_empty()).then(|| {
            let toggled = root.clone();
            ArchivedGroup {
                disclosure: if self.archive_expanded.contains(&root) {
                    Disclosure::Expanded
                } else {
                    Disclosure::Collapsed
                },
                on_toggle: Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.toggle_archived(toggled.clone(), cx);
                })),
                rows: archived.into_iter().map(|row| self.render_row(row, theme, cx)).collect(),
            }
        });
        let more = more.then(|| {
            let root = root.clone();
            cx.listener(move |this, _: &ClickEvent, _, cx| this.show_more(root.clone(), cx))
        });
        let toggled = root.clone();
        let new_thread_root = root.clone();
        ProjectSection::new(project_name(&root), theme)
            .disclosure(disclosure)
            .on_toggle(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.set_disclosure(toggled.clone(), disclosure.toggled(), cx);
            }))
            .on_new_thread(
                cx.listener(move |this, _: &ClickEvent, _, cx| this.new_thread_in(new_thread_root.clone(), cx)),
            )
            .when_some(more, ProjectSection::on_show_more)
            .when_some(archived, ProjectSection::archived)
            .children(rows)
    }

    /// The row, with a context menu of what can be done with its thread.
    fn render_row(&self, row: Row, theme: &UiTheme, cx: &mut Context<Self>) -> AnyElement {
        let Row { selector, kind, session_id, title, meta } = row;
        let editing = self.rename.as_ref().filter(|rename| session_id.as_ref() == Some(&rename.session_id));
        let mut items =
            session_id.map(|session_id| self.thread_items(session_id, &title, &meta, cx)).unwrap_or_default();
        let mut thread_row = ThreadRow::new(selector.clone(), title, theme);
        match kind {
            RowKind::Live(thread) => {
                let manager = self.threads.read(cx);
                let thread = thread.read(cx);
                let id = thread.id();
                let status = thread.status();
                let seen = if manager.is_unread(id) { Seen::Unread } else { Seen::Read };
                let (select, stop, reconnect, menu_stop) =
                    (self.threads.clone(), self.threads.clone(), self.threads.clone(), self.threads.clone());
                thread_row = thread_row
                    .dot(StatusDot::of(&status, seen))
                    .started_at(Some(thread.opened_at()))
                    .selected(manager.selected() == Some(id))
                    .on_click(move |_, _, cx| select.update(cx, |threads, cx| threads.select(id, cx)))
                    .on_stop(move |_, _, cx| stop.update(cx, |threads, cx| threads.stop(id, cx)))
                    .when(status.can_reconnect(), |row| {
                        row.on_reconnect(move |_, _, cx| reconnect.update(cx, |threads, cx| threads.reconnect(id, cx)))
                    });
                items.push(MenuItem::new("menu-stop", "Stop", move |_, cx| {
                    menu_stop.update(cx, |threads, cx| threads.stop(id, cx));
                }));
            }
            RowKind::Saved(HistoryEntry { session_id, cwd, created_at, .. }) => {
                let threads = self.threads.clone();
                thread_row =
                    thread_row.dot(Some(StatusDot::Dormant)).started_at(created_at).on_click(move |_, _, cx| {
                        let session_id = SessionId::new(session_id.as_str());
                        threads.update(cx, |threads, cx| threads.resume(session_id, cwd.clone(), cx));
                    });
            }
        }
        if let Some(Rename { input, .. }) = editing {
            thread_row = thread_row.editor(Self::render_editor(input, theme, cx));
        }
        thread_menu::with_menu(&selector, thread_row, items, theme).into_any_element()
    }

    /// The menu entries every thread with a session has, whether live or saved.
    fn thread_items(&self, session_id: SessionId, title: &str, meta: &ThreadMeta, cx: &Context<Self>) -> Vec<MenuItem> {
        let sidebar = cx.entity();
        let title = title.to_owned();
        let rename_id = session_id.clone();
        let rename = MenuItem::new("menu-rename", "Rename", move |window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.start_rename(rename_id.clone(), &title, window, cx));
        });
        let pin = if meta.pinned {
            self.registry_item("menu-unpin", "Unpin", session_id.clone(), Registry::unpin)
        } else {
            self.registry_item("menu-pin", "Pin", session_id.clone(), Registry::pin)
        };
        let archive = if meta.archived {
            self.registry_item("menu-unarchive", "Unarchive", session_id, Registry::unarchive)
        } else {
            self.registry_item("menu-archive", "Archive", session_id, Registry::archive)
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
            .rounded_sm()
            .border_1()
            .border_color(theme.colors.accent)
            .bg(theme.colors.surface)
            .on_action(cx.listener(|this, _: &Enter, _, cx| this.commit_rename(cx)))
            .on_action(cx.listener(|this, _: &Escape, _, cx| this.cancel_rename(cx)))
            .child(Input::new(input))
            .into_any_element()
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = DesktopTheme::global(cx).ui();
        let sections = self.sections(cx);
        let has_projects = !sections.is_empty();
        let sections: Vec<ProjectSection> =
            sections.into_iter().map(|section| self.render_section(section, &theme, cx)).collect();
        let projects = if has_projects {
            div().flex_1().min_h_0().child(
                div().id("projects").size_full().overflow_y_scroll().flex().flex_col().gap_1().children(sections),
            )
        } else {
            div().flex_1().min_h_0().child(EmptyState::new(
                "No projects",
                "Add a project to start a thread in it",
                NoticeTone::Neutral,
                theme,
            ))
        };
        div()
            .debug_selector(|| "sidebar".to_owned())
            .flex_none()
            .w(SIDEBAR_WIDTH)
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme.colors.border)
            .child(
                div().flex_none().p_2().child(
                    Button::new("new-thread", "New thread", theme)
                        .variant(ButtonVariant::Primary)
                        .disabled(!has_projects)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.new_thread(cx))),
                ),
            )
            .child(projects)
            .child(
                div().flex_none().p_2().border_t_1().border_color(theme.colors.border).child(
                    Button::new("add-project", "Add project", theme)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.add_project(cx))),
                ),
            )
    }
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// How many unpinned saved sessions a project lists before its "Show more" row.
const SAVED_ROWS: usize = 5;

/// How many more saved sessions each click of "Show more" reveals.
const MORE_SAVED_ROWS: usize = 20;

/// A project as the sidebar lists it. Everything but its header is empty while it is collapsed.
struct Section {
    root: PathBuf,
    disclosure: Disclosure,
    /// The rows it shows above its "Show more" row.
    rows: Vec<Row>,
    /// Whether some saved sessions are hidden behind "Show more".
    more: bool,
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
/// unpinned saved sessions are kept; and whether some were left out.
fn visible(rows: Vec<Row>, limit: usize) -> (Vec<Row>, bool) {
    let (mut visible, unpinned): (Vec<Row>, Vec<Row>) = rows.into_iter().partition(|row| row.meta.pinned);
    let mut saved = 0;
    let mut more = false;
    for row in unpinned {
        if matches!(row.kind, RowKind::Saved(_)) {
            saved += 1;
            if saved > limit {
                more = true;
                continue;
            }
        }
        visible.push(row);
    }
    (visible, more)
}

fn project_name(root: &Path) -> String {
    root.file_name().map_or_else(|| root.display().to_string(), |name| name.to_string_lossy().into_owned())
}
