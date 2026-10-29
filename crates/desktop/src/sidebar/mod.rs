mod listing;
mod row;

pub(crate) use listing::{RowKey, SwitcherEntry};

use crate::agents::{AgentProfile, Agents};
use crate::clock::Clock;
use crate::history::{History, git_root};
use crate::registry::{Collapsed, Registry};
use crate::remotes::{Action, Fleet, RemoteSpec};
use crate::theme::DesktopTheme;
use crate::threads::{Thread, ThreadManager};
use crate::ui::tokens::{ROW_HEIGHT, TWO_LINE_ROW_HEIGHT, rem};
use crate::ui::{
    Icon, IconButton, IconName, ListRow, MenuItem, PanelSide, ResizablePanel, SectionHeader, StyledExt, TreeRow,
    Typography, TypographyExt, UiTheme, icon_slot, in_top_bar, tagged, text_input, tokens, with_menu,
};
use crate::{OpenSettings, OpenThreadSwitcher};
use agent_client_protocol::schema::v2::SessionId;
use gpui::{
    AnyElement, App, ClickEvent, Context, Div, Entity, MouseButton, PathPromptOptions, Rems, SharedString,
    StyleRefinement, Subscription, Window, actions, div, prelude::*, rems,
};
use gpui_base::input::InputState;
use listing::{Listing, RemoteSection, Row, RowKind, Section, project_name};
use row::{RowShown, SidebarRow};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const MIN_MAIN_WIDTH: Rems = tokens::rem(320.0);

actions!(aether_desktop, [NewThread, NewCloudThread, ToggleSidebar,]);

pub struct Sidebar {
    threads: Entity<ThreadManager>,
    history: Entity<History>,
    registry: Entity<Registry>,
    agents: Entity<Agents>,
    fleet: Entity<Fleet>,
    clock: Arc<dyn Clock>,
    saved_limits: HashMap<PathBuf, usize>,
    rename: Option<Rename>,
    sections: Vec<Section>,
    remotes: Vec<RemoteSection>,
    rows: HashMap<RowKey, Entity<SidebarRow>>,
    _subscriptions: [Subscription; 5],
}

pub(crate) fn toggle_button(open: bool, theme: &UiTheme) -> Div {
    in_top_bar(
        PanelSide::Left
            .toggle_button("toggle-sidebar", open, "sidebar", *theme)
            .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleSidebar), cx)),
    )
}

impl Sidebar {
    pub(crate) fn new(
        threads: Entity<ThreadManager>,
        history: Entity<History>,
        registry: Entity<Registry>,
        agents: Entity<Agents>,
        fleet: Entity<Fleet>,
        clock: Arc<dyn Clock>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = [
            cx.observe(&threads, |this, _, cx| this.rebuild(cx)),
            cx.observe(&history, |this, _, cx| this.rebuild(cx)),
            cx.observe(&registry, |this, _, cx| this.rebuild(cx)),
            cx.observe(&fleet, |this, _, cx| this.rebuild(cx)),
            cx.observe(&agents, |_, _, cx| cx.notify()),
        ];
        let mut sidebar = Self {
            threads,
            history,
            registry,
            agents,
            fleet,
            clock,
            saved_limits: HashMap::new(),
            rename: None,
            sections: Vec::new(),
            remotes: Vec::new(),
            rows: HashMap::new(),
            _subscriptions: subscriptions,
        };
        (sidebar.sections, sidebar.remotes) = sidebar.build_sections(cx);
        sidebar.sync_rows(cx);
        sidebar
    }

    pub(crate) fn new_thread(&mut self, cx: &mut Context<Self>) {
        if let Some(root) = self.default_project(cx) {
            self.new_thread_in(root, cx);
        }
    }

    pub(crate) fn new_cloud_thread(&mut self, cx: &mut Context<Self>) {
        if let Some(remote) = self.default_remote(cx) {
            self.threads.update(cx, |threads, cx| drop(threads.new_cloud_thread(&remote, cx)));
        }
    }

    pub(crate) fn width(&self, cx: &App) -> Rems {
        self.registry.read(cx).state().sidebar_width.map_or(DEFAULT_SIDEBAR_WIDTH, rems)
    }

    pub(crate) fn collapsed(&self, cx: &App) -> bool {
        self.registry.read(cx).state().sidebar_collapsed
    }

    pub(crate) fn live_order(&self, cx: &App) -> Vec<Entity<Thread>> {
        let mut order: Vec<Entity<Thread>> = self.listed_rows().filter_map(Row::live).cloned().collect();
        for thread in self.threads.read(cx).threads() {
            if !order.contains(thread) {
                order.push(thread.clone());
            }
        }
        order
    }

    pub(crate) fn switcher_entries(&self, cx: &App) -> Vec<SwitcherEntry> {
        self.listing(cx).switcher_entries(&self.live_order(cx))
    }

    fn listing<'a>(&self, cx: &'a App) -> Listing<'a> {
        Listing {
            state: self.registry.read(cx).state(),
            live: self.threads.read(cx).threads(),
            history: self.history.read(cx).projects(),
            fleet: self.fleet.read(cx),
            cx,
        }
    }

    fn resize(&mut self, width: Rems, window: &Window, cx: &mut Context<Self>) {
        let available = window.viewport_size().width / window.rem_size();
        let widest = rems(available - MIN_MAIN_WIDTH.0).0.clamp(MIN_SIDEBAR_WIDTH.0, MAX_SIDEBAR_WIDTH.0);
        self.set_width(Some(rems(width.0.min(widest))), cx);
    }

    fn set_width(&mut self, width: Option<Rems>, cx: &mut Context<Self>) {
        Registry::update(&self.registry, cx, |state| state.sidebar_width = width.map(|width| width.0));
        cx.notify();
    }

    fn new_thread_in(&self, root: PathBuf, cx: &mut Context<Self>) {
        let catalog = self.agents.update(cx, |agents, _| agents.catalog(&root));
        let last = self.registry.read(cx).state().last_agents.get(&root).and_then(|name| catalog.get(name));
        let agent = last.unwrap_or_else(|| catalog.default_agent()).clone();
        self.threads.update(cx, |threads, cx| {
            threads.new_thread(root, agent, cx);
        });
    }

    fn new_thread_with(&self, root: PathBuf, agent: AgentProfile, cx: &mut Context<Self>) {
        let name = agent.name.clone();
        Registry::update(&self.registry, cx, |state| {
            state.last_agents.insert(root.clone(), name);
        });
        self.threads.update(cx, |threads, cx| {
            threads.new_thread(root, agent, cx);
        });
    }

    fn has_new_thread_menu(&self, root: &Path, cx: &mut App) -> bool {
        let catalog = self.agents.update(cx, |agents, _| agents.catalog(root));
        catalog.agents().len() > 1 || !catalog.problems().is_empty()
    }

    fn new_thread_menu(sidebar: &Entity<Self>, root: &Path, cx: &mut App) -> Vec<MenuItem> {
        let agents = sidebar.read(cx).agents.clone();
        let catalog = agents.update(cx, |agents, _| agents.catalog(root));
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
        let selected = self.threads.read(cx).selected().map(|thread| thread.read(cx));
        selected
            .and_then(|thread| thread.project().map(Path::to_path_buf))
            .or_else(|| self.sections.first().map(|section| section.root.clone()))
    }

    fn default_remote(&self, cx: &App) -> Option<RemoteSpec> {
        let inventory = self.fleet.read(cx).inventory();
        let selected = self.threads.read(cx).selected().and_then(|thread| thread.read(cx).target().remote_name());
        selected
            .and_then(|name| inventory.remote(name))
            .filter(|remote| starts_threads(remote))
            .or_else(|| inventory.remotes().iter().find(|remote| starts_threads(remote)))
            .cloned()
    }

    fn add_project(&self, root: PathBuf, cx: &mut Context<Self>) {
        Registry::update(&self.registry, cx, |state| state.add_project(root));
    }

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

    fn archive_inactive(&self, root: &Path, cx: &mut Context<Self>) {
        let inactive = self.listing(cx).inactive_sessions(root);
        Registry::update(&self.registry, cx, |state| state.archived.extend(inactive));
    }

    fn remove_project(&self, root: &Path, cx: &mut Context<Self>) {
        if self.threads.read(cx).projects(cx).all(|project| project != root) {
            Registry::update(&self.registry, cx, |state| state.projects.retain(|project| project != root));
        }
    }

    fn show_more(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        *self.saved_limits.entry(root).or_insert(listing::SAVED_ROWS) += MORE_SAVED_ROWS;
        self.rebuild(cx);
    }

    fn start_rename(&mut self, session_id: SessionId, title: &str, window: &mut Window, cx: &mut Context<Self>) {
        let input = text_input(title.to_owned(), window, cx);
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        self.rename = Some(Rename { session_id, input });
        self.sync_rows(cx);
        cx.notify();
    }

    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        if let Some(Rename { session_id, input }) = self.rename.take() {
            let title = input.read(cx).value();
            Registry::update(&self.registry, cx, |state| state.rename(session_id, &title));
            self.sync_rows(cx);
            cx.notify();
        }
    }

    fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        if self.rename.take().is_some() {
            self.sync_rows(cx);
            cx.notify();
        }
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        (self.sections, self.remotes) = self.build_sections(cx);
        self.sync_rows(cx);
        cx.notify();
    }

    fn build_sections(&self, cx: &App) -> (Vec<Section>, Vec<RemoteSection>) {
        let listing = self.listing(cx);
        (listing.sections(self.clock.now(), &self.saved_limits), listing.remotes())
    }

    fn sync_rows(&mut self, cx: &mut Context<Self>) {
        let now = self.clock.now();
        let threads = self.threads.read(cx);
        let shown: Vec<RowShown> = self
            .listed_rows()
            .map(|row| {
                let rename = self.rename.as_ref().filter(|rename| row.session_id() == Some(&rename.session_id));
                RowShown::of(row, rename.map(|rename| rename.input.clone()), threads, now, cx)
            })
            .collect();
        let mut rows = HashMap::with_capacity(shown.len());
        for shown in shown {
            let key = shown.row.key.clone();
            let view = if let Some(view) = self.rows.remove(&key) {
                view.update(cx, |view, cx| view.show(shown, cx));
                view
            } else {
                let sidebar = cx.weak_entity();
                cx.new(|_| SidebarRow::new(sidebar, shown))
            };
            rows.insert(key, view);
        }
        self.rows = rows;
    }

    fn listed_rows(&self) -> impl Iterator<Item = &Row> {
        let local = self.sections.iter().flat_map(|section| &section.rows);
        local.chain(self.remotes.iter().flat_map(|section| &section.rows))
    }

    fn render_header(theme: &UiTheme, window: &Window) -> impl IntoElement {
        div()
            .top_bar(true, window)
            .pr_2()
            .gap_1()
            .child(div().flex_1())
            .child(in_top_bar(
                IconButton::new("search-threads", IconName::Search, "Search threads", *theme)
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenThreadSwitcher), cx)),
            ))
            .child(in_top_bar(
                IconButton::new("open-settings", IconName::Settings, "Settings", *theme)
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenSettings), cx)),
            ))
            .child(toggle_button(true, theme))
    }

    fn section_header(
        &self,
        key: &'static str,
        title: &'static str,
        icon: IconName,
        theme: &UiTheme,
        cx: &App,
    ) -> SectionHeader {
        let expanded = !self.is_collapsed(&Collapsed::Section(key.to_owned()), cx);
        let registry = self.registry.clone();
        SectionHeader::new(format!("section-{key}"), title, expanded, *theme)
            .icon(icon, theme.colors.text_muted)
            .on_toggle(move |_, _, cx| {
                Registry::update(&registry, cx, |state| state.toggle_collapsed(Collapsed::Section(key.to_owned())));
            })
    }

    fn render_local(&self, theme: &UiTheme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let sidebar = cx.entity();
        let items = move |_: &mut Window, cx: &mut App| {
            let mut items: Vec<MenuItem> = sidebar
                .read(cx)
                .listing(cx)
                .unlisted_projects(RECENT_PROJECTS)
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
            items
        };
        let add_project = IconButton::new("add-project", IconName::FolderPlus, "Add project", *theme);
        let header = self
            .section_header(LOCAL, "Local", IconName::Laptop, theme, cx)
            .actions_always_visible(true)
            .action(with_menu("add-project", add_project, MouseButton::Left, items, theme));
        let mut items = vec![header.into_any_element()];
        if self.is_collapsed(&Collapsed::Section(LOCAL.to_owned()), cx) {
            return items;
        }
        if self.sections.is_empty() {
            items.push(placeholder("local-empty", "Add a project to start a thread in it", theme).into_any_element());
        }
        let sections: Vec<Div> = self.sections.iter().map(|section| self.render_section(section, theme, cx)).collect();
        items.extend(sections.into_iter().map(IntoElement::into_any_element));
        items
    }

    fn render_cloud(&self, theme: &UiTheme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let file_problem = self.fleet.read(cx).file_problem().map(str::to_owned);
        if self.remotes.is_empty() && file_problem.is_none() {
            return Vec::new();
        }
        let mut items = vec![self.section_header(CLOUD, "Cloud", IconName::Cloud, theme, cx).into_any_element()];
        if self.is_collapsed(&Collapsed::Section(CLOUD.to_owned()), cx) {
            return items;
        }
        if let Some(problem) = file_problem {
            items.push(placeholder("cloud-problem", problem, theme).into_any_element());
        }
        let remotes: Vec<Div> = self.remotes.iter().map(|section| self.render_remote(section, theme, cx)).collect();
        items.extend(remotes.into_iter().map(IntoElement::into_any_element));
        items
    }

    fn render_remote(&self, section: &RemoteSection, theme: &UiTheme, cx: &mut Context<Self>) -> Div {
        let RemoteSection { remote, collapsed, problem, rows } = section;
        let selector = format!("remote-{}", remote.name);
        let starts = starts_threads(remote);
        let new_thread = |this: &mut Self, remote: &RemoteSpec, cx: &mut Context<Self>| {
            this.threads.update(cx, |threads, cx| drop(threads.new_cloud_thread(remote, cx)));
        };
        let launch_label = format!("New thread on {}", remote.name);
        let launch = IconButton::new(format!("{selector}-new-thread"), IconName::Plus, launch_label, *theme).when(
            starts,
            |button| {
                let remote = remote.clone();
                button.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| new_thread(this, &remote, cx)))
            },
        );
        let (fleet, file) = (self.fleet.clone(), self.fleet.read(cx).file().map(Path::to_path_buf));
        let header = Header {
            selector: &selector,
            label: remote.name.clone(),
            icon: (format!("{selector}-icon"), IconName::Cpu),
            group: Collapsed::Remote(remote.name.clone()),
            collapsed: *collapsed,
            actions: ("Remote actions", Box::new(move |_, _| remote_actions(&fleet, file.as_deref()))),
            new_thread: launch.into_any_element(),
        };
        let section = self.render_group(header, theme);
        if *collapsed {
            return section;
        }
        if let Some(problem) = problem {
            return section.child(placeholder(format!("{selector}-problem"), problem.clone(), theme));
        }
        if rows.is_empty() {
            let text = match (remote.is_static(), starts) {
                (true, _) => "No threads. Start one…",
                (false, true) => "Nothing running. Launch one…",
                (false, false) => "Nothing running",
            };
            let remote = remote.clone();
            let empty = placeholder(format!("{selector}-empty"), text, theme).when(starts, |empty| {
                empty.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| new_thread(this, &remote, cx)))
            });
            return section.child(empty);
        }
        section.children(rows.iter().filter_map(|row| self.render_row(row)))
    }

    fn render_section(&self, section: &Section, theme: &UiTheme, cx: &mut Context<Self>) -> Div {
        let Section { root, collapsed, rows, hidden } = section;
        let (name, muted) = (project_name(root), theme.colors.text_muted);
        let selector = format!("project-{name}");
        let new_thread_selector = format!("{selector}-new-thread");
        let new_thread =
            IconButton::new(new_thread_selector.clone(), IconName::Plus, format!("New thread in {name}"), *theme);
        let new_thread = if self.has_new_thread_menu(root, cx) {
            let (sidebar, root) = (cx.entity(), root.clone());
            let menu = move |_: &mut Window, cx: &mut App| Self::new_thread_menu(&sidebar, &root, cx);
            with_menu(&new_thread_selector, new_thread, MouseButton::Left, menu, theme).into_any_element()
        } else {
            let root = root.clone();
            new_thread
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.new_thread_in(root.clone(), cx)))
                .into_any_element()
        };
        let (sidebar, actions_root) = (cx.entity(), root.clone());
        let (folder, state) = if *collapsed { (IconName::Folder, "closed") } else { (IconName::FolderOpen, "open") };
        let header = Header {
            selector: &selector,
            label: name,
            icon: (format!("{selector}-folder-{state}"), folder),
            group: Collapsed::Project(root.clone()),
            collapsed: *collapsed,
            actions: ("Project actions", Box::new(move |_, _| project_actions(&sidebar, &actions_root))),
            new_thread,
        };
        let section = self.render_group(header, theme);
        if *collapsed {
            return section;
        }
        let rows: Vec<AnyElement> = rows.iter().filter_map(|row| self.render_row(row)).collect();
        let show_more = (*hidden > 0).then(|| {
            let root = root.clone();
            TreeRow::new(SharedString::from(format!("{selector}-show-more")), format!("Show {hidden} more"), *theme)
                .depth(1)
                .leading(icon_slot())
                .label_color(muted)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.show_more(root.clone(), cx)))
        });
        section.children(rows).children(show_more)
    }

    fn render_group(&self, header: Header<'_>, theme: &UiTheme) -> Div {
        let Header { selector, label, icon: (icon_selector, icon), group, collapsed, actions, new_thread } = header;
        let muted = theme.colors.text_muted;
        let actions_selector = format!("{selector}-actions");
        let (actions_label, items) = actions;
        let actions = with_menu(
            &actions_selector,
            IconButton::new(actions_selector.clone(), IconName::Ellipsis, actions_label, *theme),
            MouseButton::Left,
            items,
            theme,
        );
        let registry = self.registry.clone();
        let header = TreeRow::new(SharedString::from(selector.to_owned()), label, *theme)
            .expanded(!collapsed)
            .leading(tagged(icon_selector, Icon::new(icon, muted)))
            .label_color(muted)
            .on_click(move |_: &ClickEvent, _, cx| {
                Registry::update(&registry, cx, |state| state.toggle_collapsed(group.clone()));
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .reveal_on_hover(ListRow::hover_group(selector))
                    .child(actions)
                    .child(new_thread),
            );
        div().w_full().flex().flex_col().child(header)
    }

    fn render_row(&self, row: &Row) -> Option<AnyElement> {
        let view = self.rows.get(&row.key)?.clone();
        let height = match row.kind {
            RowKind::Local { .. } => ROW_HEIGHT,
            RowKind::Cloud { .. } => TWO_LINE_ROW_HEIGHT,
        };
        Some(view.cached(StyleRefinement::default().w_full().h(rem(height))).into_any_element())
    }

    fn is_collapsed(&self, group: &Collapsed, cx: &App) -> bool {
        self.registry.read(cx).state().collapsed.contains(group)
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = DesktopTheme::global(cx).ui();
        let sidebar = cx.entity().downgrade();
        let reset = sidebar.clone();
        let local = self.render_local(&theme, cx);
        let cloud = self.render_cloud(&theme, cx);
        let lists = div().flex_1().min_h_0().child(
            div()
                .id("projects")
                .size_full()
                .overflow_y_scroll()
                .px_2()
                .pb_2()
                .flex()
                .flex_col()
                .gap_1()
                .children(local)
                .when(!cloud.is_empty(), |lists| lists.child(div().h_2()))
                .children(cloud),
        );
        ResizablePanel::new("sidebar", PanelSide::Left, self.width(cx), theme)
            .min_width(MIN_SIDEBAR_WIDTH)
            .max_width(MAX_SIDEBAR_WIDTH)
            .on_resize(move |width, window, cx| {
                let _ = sidebar.update(cx, |sidebar, cx| sidebar.resize(width, window, cx));
            })
            .on_reset(move |_, cx| {
                let _ = reset.update(cx, |sidebar, cx| sidebar.set_width(None, cx));
            })
            .bg(theme.colors.chrome)
            .child(Self::render_header(&theme, window))
            .child(lists)
    }
}

const DEFAULT_SIDEBAR_WIDTH: Rems = tokens::rem(272.0);
const MIN_SIDEBAR_WIDTH: Rems = tokens::rem(200.0);
const MAX_SIDEBAR_WIDTH: Rems = tokens::rem(480.0);

const MORE_SAVED_ROWS: usize = 20;

const RECENT_PROJECTS: usize = 8;

const LOCAL: &str = "local";
const CLOUD: &str = "cloud";

struct Rename {
    session_id: SessionId,
    input: Entity<InputState>,
}

struct Header<'a> {
    selector: &'a str,
    label: String,
    icon: (String, IconName),
    group: Collapsed,
    collapsed: bool,
    actions: (&'static str, MenuItems),
    new_thread: AnyElement,
}

type MenuItems = Box<dyn FnOnce(&mut Window, &mut App) -> Vec<MenuItem>>;

fn project_actions(sidebar: &Entity<Sidebar>, root: &Path) -> Vec<MenuItem> {
    let sidebar = sidebar.clone();
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

fn remote_actions(fleet: &Entity<Fleet>, file: Option<&Path>) -> Vec<MenuItem> {
    let fleet = fleet.clone();
    let refresh = MenuItem::new("menu-refresh-instances", "Refresh", move |_, cx| fleet.update(cx, Fleet::refresh))
        .icon(IconName::RotateCw);
    let open = file.map(Path::to_path_buf).map(|file| {
        MenuItem::new("menu-open-remotes-file", "Open remotes.json", move |_, cx| cx.open_with_system(&file))
            .icon(IconName::FileText)
    });
    std::iter::once(refresh).chain(open).collect()
}

fn starts_threads(remote: &RemoteSpec) -> bool {
    remote.is_static() || remote.can(Action::Launch)
}

fn placeholder(selector: impl Into<SharedString>, text: impl Into<SharedString>, theme: &UiTheme) -> ListRow {
    let text = text.into();
    ListRow::new(selector.into(), *theme).depth(1).child(icon_slot()).child(
        div().flex_1().min_w_0().py_1().typography(Typography::Caption).text_color(theme.colors.text_muted).child(text),
    )
}
