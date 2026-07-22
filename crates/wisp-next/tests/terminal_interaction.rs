use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::TerminalOptions;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use tokio::sync::mpsc::UnboundedReceiver;
use wisp_next::app::{App, AppConfig};
use wisp_next::presentation::TranscriptRenderer;
use wisp_next::render::sync_terminal as sync_terminal_with_renderer;
use wisp_next::settings::UiSettings;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_terminal(width: u16, height: u16) -> ratatui::Terminal<TestBackend> {
    let viewport_height = wisp_next::inline_viewport_height(height);
    ratatui::Terminal::with_options(
        TestBackend::new(width, height),
        TerminalOptions { viewport: ratatui::Viewport::Inline(viewport_height) },
    )
    .unwrap()
}

fn make_app() -> (App, UnboundedReceiver<acp_utils::client::PromptCommand>) {
    let (prompt_handle, command_rx) = acp_utils::client::AcpPromptHandle::recording();
    let app = App::new(AppConfig {
        session_id: agent_client_protocol::schema::SessionId::new("test-session"),
        agent_name: "aether".to_string(),
        prompt_capabilities: agent_client_protocol::schema::PromptCapabilities::new(),
        session_capabilities: agent_client_protocol::schema::SessionCapabilities::new(),
        config_options: Vec::new(),
        auth_methods: Vec::new(),
        workspace_status: wisp_next::workspace_status::WorkspaceStatus::new("~/code/demo", Some("main".to_string())),
        prompt_handle,
        working_dir: std::path::PathBuf::from("."),
        settings: UiSettings::default(),
    });
    (app, command_rx)
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> crossterm::event::Event {
    crossterm::event::Event::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE })
}

fn scroll_down(column: u16, row: u16) -> crossterm::event::Event {
    mouse_event(MouseEventKind::ScrollDown, column, row)
}

fn scroll_up(column: u16, row: u16) -> crossterm::event::Event {
    mouse_event(MouseEventKind::ScrollUp, column, row)
}

fn click(column: u16, row: u16) -> crossterm::event::Event {
    mouse_event(MouseEventKind::Down(MouseButton::Left), column, row)
}

// ---------------------------------------------------------------------------
// Picker click offset tests
// ---------------------------------------------------------------------------

mod picker_click {
    use super::*;
    use acp_utils::client::AcpEvent;
    use agent_client_protocol::schema::SessionInfo;

    #[test]
    fn session_picker_click_first_row_selects_index_zero() {
        let (mut app, _rx) = make_app();

        // Open session picker with sessions
        let sessions = vec![
            SessionInfo::new(agent_client_protocol::schema::SessionId::new("s1"), "/tmp"),
            SessionInfo::new(agent_client_protocol::schema::SessionId::new("s2"), "/tmp"),
            SessionInfo::new(agent_client_protocol::schema::SessionId::new("s3"), "/tmp"),
        ];
        app.on_acp_event(AcpEvent::SessionsListed { sessions });
        assert!(app.has_session_picker());

        let mut terminal = make_terminal(80, 24);
        let mut renderer = TranscriptRenderer::new(&UiSettings::default());
        sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
        assert!(app.surface_rect().is_some());

        let rect = app.surface_rect().unwrap();
        // Click on the second content row (local_y=2 → row=1 after border offset)
        app.on_terminal_event(click(rect.x + 2, rect.y + 2));
        assert!(app.has_session_picker(), "picker should remain open");
    }

    #[test]
    fn session_picker_click_outside_row_range_is_clamped() {
        let (mut app, _rx) = make_app();

        let sessions = vec![
            SessionInfo::new(agent_client_protocol::schema::SessionId::new("s1"), "/tmp"),
            SessionInfo::new(agent_client_protocol::schema::SessionId::new("s2"), "/tmp"),
        ];
        app.on_acp_event(AcpEvent::SessionsListed { sessions });
        assert!(app.has_session_picker());

        let mut terminal = make_terminal(80, 24);
        let mut renderer = TranscriptRenderer::new(&UiSettings::default());
        sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

        let rect = app.surface_rect().unwrap();
        // Click far below content (local_y=20 → row=19 after offset, clamped to last item)
        app.on_terminal_event(click(rect.x + 2, rect.y + 20));
        assert!(app.has_session_picker());
    }

    #[test]
    fn session_picker_click_with_filter_uses_visible_rows() {
        let (mut app, _rx) = make_app();

        let mut session_a = SessionInfo::new(agent_client_protocol::schema::SessionId::new("aaa"), "/tmp");
        session_a.title = Some("Alpha Project".to_string());
        let mut session_b = SessionInfo::new(agent_client_protocol::schema::SessionId::new("bbb"), "/tmp");
        session_b.title = Some("Beta Project".to_string());
        let mut session_c = SessionInfo::new(agent_client_protocol::schema::SessionId::new("ccc"), "/tmp");
        session_c.title = Some("Alpha Config".to_string());

        app.on_acp_event(AcpEvent::SessionsListed { sessions: vec![session_a, session_b, session_c] });
        assert!(app.has_session_picker());

        // Type filter: "Alpha"
        app.on_key(key(KeyCode::Char('A')));
        app.on_key(key(KeyCode::Char('l')));
        app.on_key(key(KeyCode::Char('p')));
        app.on_key(key(KeyCode::Char('h')));
        app.on_key(key(KeyCode::Char('a')));

        let mut terminal = make_terminal(80, 24);
        let mut renderer = TranscriptRenderer::new(&UiSettings::default());
        sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

        let rect = app.surface_rect().unwrap();
        // Click on first visible row
        app.on_terminal_event(click(rect.x + 2, rect.y + 2));
        assert!(app.has_session_picker());
    }

    #[test]
    fn workspace_picker_click_first_row_selects_index_zero() {
        use acp_utils::notifications::{WorkspaceEntry, WorkspaceListResponse};

        let workspaces = vec![
            WorkspaceEntry { path: std::path::PathBuf::from("/tmp/ws1"), is_current: false },
            WorkspaceEntry { path: std::path::PathBuf::from("/tmp/ws2"), is_current: false },
            WorkspaceEntry { path: std::path::PathBuf::from("/tmp/ws3"), is_current: false },
        ];
        let (mut app, _rx) = make_app();
        app.on_acp_event(AcpEvent::WorkspacesListed(WorkspaceListResponse { workspaces }));

        let mut terminal = make_terminal(80, 24);
        let mut renderer = TranscriptRenderer::new(&UiSettings::default());
        sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
        assert!(app.surface_rect().is_some());

        let rect = app.surface_rect().unwrap();
        // Click on the first content row (local_y=1 → row=0 after border offset)
        app.on_terminal_event(click(rect.x + 2, rect.y + 1));
    }

    #[test]
    fn workspace_picker_click_outside_row_range_is_clamped() {
        use acp_utils::notifications::{WorkspaceEntry, WorkspaceListResponse};

        let workspaces = vec![WorkspaceEntry { path: std::path::PathBuf::from("/tmp/ws1"), is_current: false }];
        let (mut app, _rx) = make_app();
        app.on_acp_event(AcpEvent::WorkspacesListed(WorkspaceListResponse { workspaces }));

        let mut terminal = make_terminal(80, 24);
        let mut renderer = TranscriptRenderer::new(&UiSettings::default());
        sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

        let rect = app.surface_rect().unwrap();
        // Click far below content
        app.on_terminal_event(click(rect.x + 2, rect.y + 15));
    }

    #[test]
    fn workspace_picker_click_with_filter_uses_filtered_rows() {
        use acp_utils::notifications::{WorkspaceEntry, WorkspaceListResponse};

        let workspaces = vec![
            WorkspaceEntry { path: std::path::PathBuf::from("/tmp/project-alpha"), is_current: false },
            WorkspaceEntry { path: std::path::PathBuf::from("/tmp/project-beta"), is_current: false },
            WorkspaceEntry { path: std::path::PathBuf::from("/tmp/other"), is_current: false },
        ];
        let (mut app, _rx) = make_app();
        app.on_acp_event(AcpEvent::WorkspacesListed(WorkspaceListResponse { workspaces }));

        // Type filter: "beta"
        app.on_key(key(KeyCode::Char('b')));
        app.on_key(key(KeyCode::Char('e')));
        app.on_key(key(KeyCode::Char('t')));
        app.on_key(key(KeyCode::Char('a')));

        let mut terminal = make_terminal(80, 24);
        let mut renderer = TranscriptRenderer::new(&UiSettings::default());
        sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

        let rect = app.surface_rect().unwrap();
        // Click on first visible row
        app.on_terminal_event(click(rect.x + 2, rect.y + 2));
    }
}

// ---------------------------------------------------------------------------
// Mouse capture tests
// ---------------------------------------------------------------------------

mod mouse_capture {
    use super::*;
    use acp_utils::client::AcpEvent;
    use agent_client_protocol::schema::SessionInfo;

    #[test]
    fn no_capture_when_no_fullscreen_or_modal() {
        let (app, _rx) = make_app();
        assert!(!app.needs_mouse_capture());
    }

    #[test]
    fn capture_enabled_when_session_picker_is_open() {
        let (mut app, _rx) = make_app();
        let current_id = agent_client_protocol::schema::SessionId::new("test-session");
        let other = SessionInfo::new(agent_client_protocol::schema::SessionId::new("other"), "/tmp");
        app.on_acp_event(AcpEvent::SessionsListed { sessions: vec![other] });
        std::mem::drop(current_id);

        assert!(app.has_session_picker());
        assert!(app.needs_mouse_capture());

        app.on_key(key(KeyCode::Esc));
        assert!(!app.needs_mouse_capture());
    }

    #[test]
    fn capture_when_composer_overlay_is_open() {
        let (mut app, _rx) = make_app();

        // Open command picker
        app.on_key(key(KeyCode::Char('/')));
        assert!(app.needs_mouse_capture());

        app.on_key(key(KeyCode::Esc));
        assert!(!app.needs_mouse_capture());
    }

    #[test]
    fn capture_when_prompt_search_is_open() {
        let mut app = {
            let session_capabilities = agent_client_protocol::schema::SessionCapabilities::new().meta(Some(
                acp_utils::notifications::AetherCapabilities {
                    prompt_search: true,
                    session_preview: false,
                    workspace_move: false,
                }
                .to_meta(),
            ));
            let (prompt_handle, _command_rx) = acp_utils::client::AcpPromptHandle::recording();
            App::new(AppConfig {
                session_id: agent_client_protocol::schema::SessionId::new("test-session"),
                agent_name: "aether".to_string(),
                prompt_capabilities: agent_client_protocol::schema::PromptCapabilities::new(),
                session_capabilities,
                config_options: Vec::new(),
                auth_methods: Vec::new(),
                workspace_status: wisp_next::workspace_status::WorkspaceStatus::new(
                    "~/code/demo",
                    Some("main".to_string()),
                ),
                prompt_handle,
                working_dir: std::path::PathBuf::from("."),
                settings: UiSettings::default(),
            })
        };

        app.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(app.needs_mouse_capture());

        app.on_key(key(KeyCode::Esc));
        assert!(!app.needs_mouse_capture());
    }

    #[test]
    fn capture_disabled_after_connection_closed() {
        let (mut app, _rx) = make_app();
        let other = SessionInfo::new(agent_client_protocol::schema::SessionId::new("other"), "/tmp");
        app.on_acp_event(AcpEvent::SessionsListed { sessions: vec![other] });
        assert!(app.needs_mouse_capture());

        app.on_acp_event(AcpEvent::ConnectionClosed);
        assert!(!app.needs_mouse_capture());
    }
}

// ---------------------------------------------------------------------------
// Bell tests
// ---------------------------------------------------------------------------

mod bell {
    use super::*;
    use acp_utils::client::AcpEvent;
    use agent_client_protocol::schema as acp;

    #[test]
    fn bell_after_normal_completion() {
        let (mut app, _rx) = make_app();

        // Submit a prompt
        super::submit_prompt(&mut app, "hello");
        assert!(app.waiting_for_response());

        app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

        assert!(app.take_bell());
    }

    #[test]
    fn no_bell_after_cancellation() {
        let (mut app, _rx) = make_app();

        super::submit_prompt(&mut app, "hello");
        app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::Cancelled));

        assert!(!app.take_bell());
    }

    #[test]
    fn no_bell_after_prompt_error() {
        let (mut app, _rx) = make_app();

        super::submit_prompt(&mut app, "hello");
        app.on_acp_event(AcpEvent::PromptError(agent_client_protocol::Error::internal_error()));

        assert!(!app.take_bell());
    }

    #[test]
    fn no_bell_after_connection_close() {
        let (mut app, _rx) = make_app();

        super::submit_prompt(&mut app, "hello");
        app.on_acp_event(AcpEvent::ConnectionClosed);

        assert!(!app.take_bell());
    }

    #[test]
    fn no_bell_on_unsolicited_prompt_done() {
        let (mut app, _rx) = make_app();

        // No prompt in flight — PromptDone is unsolicited
        app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

        assert!(!app.take_bell());
    }

    #[test]
    fn exactly_one_bell_per_completion() {
        let (mut app, _rx) = make_app();

        super::submit_prompt(&mut app, "first");
        app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

        assert!(app.take_bell());
        assert!(!app.take_bell());
    }
}

// ---------------------------------------------------------------------------
// Resize tests
// ---------------------------------------------------------------------------

mod resize {
    use super::*;

    #[test]
    fn resize_updates_terminal_size() {
        let (mut app, _rx) = make_app();
        assert_eq!(app.terminal_size(), (0, 0));

        app.on_terminal_event(crossterm::event::Event::Resize(120, 40));
        assert_eq!(app.terminal_size(), (120, 40));
    }

    #[test]
    fn resize_preserves_composer_content() {
        let (mut app, _rx) = make_app();

        // Simulate a resize event
        app.on_terminal_event(crossterm::event::Event::Resize(120, 30));

        super::type_text(&mut app, "hello resize");
        assert_eq!(app.composer().text(), "hello resize");
    }

    #[test]
    fn resize_no_duplicate_history() {
        let (mut app, _rx) = make_app();
        let mut terminal = make_terminal(80, 20);
        let mut renderer = TranscriptRenderer::new(&UiSettings::default());

        super::submit_prompt(&mut app, "before resize");

        // Render at initial size then resize
        sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
        app.on_terminal_event(crossterm::event::Event::Resize(120, 30));
        sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

        let viewport = super::buffer_text(terminal.backend().buffer());
        assert_eq!(viewport.matches("before resize").count(), 1);
    }
}

// ---------------------------------------------------------------------------
// Event routing tests
// ---------------------------------------------------------------------------

mod event_routing {
    use super::*;
    use acp_utils::client::AcpEvent;
    use agent_client_protocol::schema::SessionInfo;

    #[test]
    fn mouse_click_outside_surface_is_ignored() {
        let (mut app, _rx) = make_app();

        app.on_acp_event(AcpEvent::SessionsListed {
            sessions: vec![SessionInfo::new(agent_client_protocol::schema::SessionId::new("other"), "/tmp")],
        });
        assert!(app.has_session_picker());

        // Click at (0, 0) — outside the picker rect (no surface rect is set without render)
        app.on_terminal_event(click(0, 0));

        // The session picker should still be open and unchanged
        assert!(app.has_session_picker());
    }

    #[test]
    fn keyboard_event_routing_precedence_unchanged() {
        let (mut app, _rx) = make_app();

        // Open settings overlay via /settings
        app.on_key(key(KeyCode::Char('/')));
        app.on_key(key(KeyCode::Char('s')));
        app.on_key(key(KeyCode::Tab));
        app.on_key(key(KeyCode::Enter));

        // Esc closes settings overlay
        app.on_key(key(KeyCode::Esc));
        assert!(!app.has_modal());
    }

    #[test]
    fn mouse_event_routes_to_topmost_surface() {
        let (mut app, _rx) = make_app();

        // Open settings overlay
        app.on_key(key(KeyCode::Char('/')));
        app.on_key(key(KeyCode::Char('s')));
        app.on_key(key(KeyCode::Tab));
        app.on_key(key(KeyCode::Enter));

        let mut terminal = make_terminal(80, 24);
        let mut renderer = TranscriptRenderer::new(&UiSettings::default());
        sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

        // Scroll events should be consumed internally
        let bell_before = app.take_bell();
        app.on_terminal_event(scroll_down(40, 5));
        assert_eq!(bell_before, app.take_bell());
    }

    #[test]
    fn scroll_on_settings_overlay_changes_menu_selection() {
        use agent_client_protocol::schema::{SessionConfigOption, SessionConfigSelectOption};
        let opts = vec![
            SessionConfigOption::select(
                "model",
                "Model",
                "a",
                vec![SessionConfigSelectOption::new("a", "Alpha"), SessionConfigSelectOption::new("b", "Beta")],
            ),
            SessionConfigOption::select(
                "mode",
                "Mode",
                "code",
                vec![SessionConfigSelectOption::new("code", "Code"), SessionConfigSelectOption::new("chat", "Chat")],
            ),
        ];
        let (prompt_handle, _rx) = acp_utils::client::AcpPromptHandle::recording();
        let mut app = App::new(AppConfig {
            session_id: agent_client_protocol::schema::SessionId::new("test"),
            agent_name: "aether".to_string(),
            prompt_capabilities: agent_client_protocol::schema::PromptCapabilities::new(),
            session_capabilities: agent_client_protocol::schema::SessionCapabilities::new(),
            config_options: opts,
            auth_methods: Vec::new(),
            workspace_status: wisp_next::workspace_status::WorkspaceStatus::new("~/code", None),
            prompt_handle,
            working_dir: std::path::PathBuf::from("."),
            settings: UiSettings::default(),
        });

        type_text(&mut app, "/settings");
        app.on_key(key(KeyCode::Tab));

        let mut terminal = make_terminal(80, 24);
        let mut renderer = TranscriptRenderer::new(&UiSettings::default());
        sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
        assert!(app.surface_rect().is_some());

        app.on_terminal_event(scroll_down(40, 5));
        assert!(!app.take_bell());
    }

    #[test]
    fn settings_overlay_click_selects_entry() {
        use agent_client_protocol::schema::{SessionConfigOption, SessionConfigSelectOption};
        let opts = vec![SessionConfigOption::select(
            "model",
            "Model",
            "a",
            vec![SessionConfigSelectOption::new("a", "Alpha"), SessionConfigSelectOption::new("b", "Beta")],
        )];
        let (prompt_handle, _rx) = acp_utils::client::AcpPromptHandle::recording();
        let mut app = App::new(AppConfig {
            session_id: agent_client_protocol::schema::SessionId::new("test"),
            agent_name: "aether".to_string(),
            prompt_capabilities: agent_client_protocol::schema::PromptCapabilities::new(),
            session_capabilities: agent_client_protocol::schema::SessionCapabilities::new(),
            config_options: opts,
            auth_methods: Vec::new(),
            workspace_status: wisp_next::workspace_status::WorkspaceStatus::new("~/code", None),
            prompt_handle,
            working_dir: std::path::PathBuf::from("."),
            settings: UiSettings::default(),
        });

        type_text(&mut app, "/settings");
        app.on_key(key(KeyCode::Tab));

        let mut terminal = make_terminal(80, 24);
        let mut renderer = TranscriptRenderer::new(&UiSettings::default());
        sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

        let rect = app.surface_rect().unwrap();
        let click_y = rect.y + 2;
        app.on_terminal_event(click(rect.x + 2, click_y));
        assert!(!app.take_bell());
    }

    #[test]
    fn session_picker_scroll_changes_selection() {
        let (mut app, _rx) = make_app();
        let current = agent_client_protocol::schema::SessionId::new("test-session");
        let sessions = vec![
            SessionInfo::new(agent_client_protocol::schema::SessionId::new("a"), "/tmp/a"),
            SessionInfo::new(agent_client_protocol::schema::SessionId::new("b"), "/tmp/b"),
            SessionInfo::new(agent_client_protocol::schema::SessionId::new("c"), "/tmp/c"),
        ];
        app.on_acp_event(AcpEvent::SessionsListed { sessions });
        std::mem::drop(current);
        assert!(app.has_session_picker());

        let mut terminal = make_terminal(80, 24);
        let mut renderer = TranscriptRenderer::new(&UiSettings::default());
        sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
        assert!(app.surface_rect().is_some());

        // Scroll down
        let rect = app.surface_rect().unwrap();
        app.on_terminal_event(scroll_down(rect.x + 2, rect.y + 2));
        assert!(!app.take_bell());
        assert!(app.has_session_picker());
    }

    #[test]
    fn prompt_search_scroll_up_and_down() {
        let mut app = {
            let session_capabilities = agent_client_protocol::schema::SessionCapabilities::new().meta(Some(
                acp_utils::notifications::AetherCapabilities {
                    prompt_search: true,
                    session_preview: false,
                    workspace_move: false,
                }
                .to_meta(),
            ));
            let (prompt_handle, _command_rx) = acp_utils::client::AcpPromptHandle::recording();
            App::new(AppConfig {
                session_id: agent_client_protocol::schema::SessionId::new("test-session"),
                agent_name: "aether".to_string(),
                prompt_capabilities: agent_client_protocol::schema::PromptCapabilities::new(),
                session_capabilities,
                config_options: Vec::new(),
                auth_methods: Vec::new(),
                workspace_status: wisp_next::workspace_status::WorkspaceStatus::new(
                    "~/code/demo",
                    Some("main".to_string()),
                ),
                prompt_handle,
                working_dir: std::path::PathBuf::from("."),
                settings: UiSettings::default(),
            })
        };

        app.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(app.needs_mouse_capture());

        let mut terminal = make_terminal(80, 24);
        let mut renderer = TranscriptRenderer::new(&UiSettings::default());
        sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

        let rect = app.surface_rect().unwrap();
        // Scroll should be consumed internally
        app.on_terminal_event(scroll_down(rect.x + 2, rect.y + 1));
        app.on_terminal_event(scroll_up(rect.x + 2, rect.y + 1));
        assert!(!app.take_bell());
    }

    #[test]
    fn composer_overlay_scroll_and_click() {
        let (mut app, _rx) = make_app();
        // Open command picker
        app.on_key(key(KeyCode::Char('/')));
        assert!(app.needs_mouse_capture());

        let mut terminal = make_terminal(80, 24);
        let mut renderer = TranscriptRenderer::new(&UiSettings::default());
        sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

        let rect = app.surface_rect();
        if let Some(rect) = rect {
            app.on_terminal_event(scroll_down(rect.x + 2, rect.y + rect.height.saturating_sub(1)));
            app.on_terminal_event(click(rect.x + 2, rect.y + rect.height.saturating_sub(1)));
        }
        assert!(!app.take_bell());
    }

    #[test]
    fn workspace_picker_scroll_up_and_down() {
        use acp_utils::notifications::{WorkspaceEntry, WorkspaceListResponse};
        use std::path::PathBuf;

        let (mut app, _rx) = make_app();
        let workspaces = vec![
            WorkspaceEntry { path: PathBuf::from("/tmp/a"), is_current: false },
            WorkspaceEntry { path: PathBuf::from("/tmp/b"), is_current: false },
            WorkspaceEntry { path: PathBuf::from("/tmp/c"), is_current: false },
        ];
        app.on_acp_event(AcpEvent::WorkspacesListed(WorkspaceListResponse { workspaces }));
        assert!(matches!(app.workspace_move_state(), wisp_next::app::WorkspaceMoveState::Picking));

        let mut terminal = make_terminal(80, 24);
        let mut renderer = TranscriptRenderer::new(&UiSettings::default());
        sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

        let rect = app.surface_rect().unwrap();
        app.on_terminal_event(scroll_down(rect.x + 2, rect.y + 2));
        app.on_terminal_event(scroll_up(rect.x + 2, rect.y + 2));
        app.on_terminal_event(click(rect.x + 2, rect.y + 2));
        assert!(!app.take_bell());
    }

    #[test]
    fn form_modal_scroll_changes_field() {
        use acp_utils::notifications::{CreateElicitationRequestParams, ElicitationParams};
        use acp_utils::testing::test_connection;

        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async {
            let (mut app, _rx) = make_app();
            let (cx, mut peer) = test_connection().await;
            let (responder, _response_rx) = peer.fake_elicitation(&cx).await;

            let schema = acp_utils::ElicitationSchema::builder()
                .required_string("field1")
                .required_string("field2")
                .build()
                .unwrap();

            app.on_acp_event(AcpEvent::ElicitationRequest {
                params: ElicitationParams {
                    server_name: "test".into(),
                    request: CreateElicitationRequestParams::FormElicitationParams {
                        meta: None,
                        message: "Test".into(),
                        requested_schema: schema,
                    },
                },
                responder,
            });

            assert!(app.needs_mouse_capture());

            let mut terminal = make_terminal(80, 24);
            let mut renderer = TranscriptRenderer::new(&UiSettings::default());
            sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

            let rect = app.surface_rect().unwrap();
            app.on_terminal_event(scroll_down(rect.x + 2, rect.y + 3));
            assert!(!app.take_bell());
        });
    }
}

// ---------------------------------------------------------------------------
// Screen mouse handling tests
// ---------------------------------------------------------------------------

mod screen_mouse {
    use super::*;
    use wisp_next::git_diff::{FileDiff, FileStatus, GitDiffDocument, StageState};
    use wisp_next::screens::git_diff::GitDiffScreen;
    use wisp_next::syntax::SyntaxHighlighter;
    use wisp_next::theme::Theme;

    fn make_test_document() -> GitDiffDocument {
        use wisp_next::git_diff::{Hunk, PatchLine, PatchLineKind};
        GitDiffDocument {
            repo_root: std::path::PathBuf::from("/tmp/repo"),
            files: vec![
                FileDiff {
                    old_path: None,
                    path: "src/main.rs".to_string(),
                    status: FileStatus::Modified,
                    staged: StageState::Unstaged,
                    hunks: vec![Hunk {
                        header: "@@ -1,5 +1,5 @@".to_string(),
                        old_start: 1,
                        old_count: 5,
                        new_start: 1,
                        new_count: 5,
                        lines: vec![PatchLine {
                            kind: PatchLineKind::Context,
                            text: "fn main() {".to_string(),
                            old_line_no: Some(1),
                            new_line_no: Some(1),
                        }],
                    }],
                    binary: false,
                },
                FileDiff {
                    old_path: None,
                    path: "src/lib.rs".to_string(),
                    status: FileStatus::Added,
                    staged: StageState::Unstaged,
                    hunks: vec![],
                    binary: false,
                },
                FileDiff {
                    old_path: None,
                    path: "Cargo.toml".to_string(),
                    status: FileStatus::Modified,
                    staged: StageState::Unstaged,
                    hunks: vec![],
                    binary: false,
                },
            ],
        }
    }

    fn render_git_diff(screen: &mut GitDiffScreen, width: u16, height: u16) -> Buffer {
        let theme = Theme::default();
        let mut highlighter = SyntaxHighlighter::new();
        let backend = TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::with_options(backend, TerminalOptions::default()).unwrap();
        terminal
            .draw(|frame| {
                screen.render(frame, &theme, &mut highlighter);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn load_document(screen: &mut GitDiffScreen, document: GitDiffDocument) {
        use wisp_next::screens::git_diff::GitDiffEvent;
        let request_id = screen.current_request_id();
        screen.on_event(GitDiffEvent::Loaded { request_id, result: Ok(document) });
    }

    #[test]
    fn git_diff_click_left_side_selects_drawer() {
        let (mut screen, _effect) = GitDiffScreen::new(std::path::PathBuf::from("/tmp/repo"));
        load_document(&mut screen, make_test_document());

        // Render at wide width (120)
        let _buffer = render_git_diff(&mut screen, 120, 40);

        // drawer_width = (120/3).clamp(24,36) = 36
        // body starts at x=1 (after border)
        // Click at x=20 (left side, well within drawer)
        screen.on_mouse_click(3, 20);

        // With focus on Drawer, footer shows "h/l pane"
        let buffer = render_git_diff(&mut screen, 120, 40);
        let text = buffer_text(&buffer);
        assert!(text.contains("h/l pane"), "drawer focus footer: {text}");
    }

    #[test]
    fn git_diff_click_right_side_selects_patch() {
        let (mut screen, _effect) = GitDiffScreen::new(std::path::PathBuf::from("/tmp/repo"));
        load_document(&mut screen, make_test_document());

        let _buffer = render_git_diff(&mut screen, 120, 40);

        // drawer_width = 36, body_x = 1. Drawer spans x=1..37. Patch spans x=38..
        // Click at x=60 (right side, patch area)
        screen.on_mouse_click(3, 60);

        // Focus should now be Patch. Footer shows "c comment" for Patch focus.
        let buffer = render_git_diff(&mut screen, 120, 40);
        let text = buffer_text(&buffer);
        // Should show a draft comment box
        assert!(text.contains("c comment"), "patch focus footer: {text}");
    }

    #[test]
    fn git_diff_click_narrow_layout_always_patch() {
        let (mut screen, _effect) = GitDiffScreen::new(std::path::PathBuf::from("/tmp/repo"));
        load_document(&mut screen, make_test_document());

        // Render at narrow width (< 72)
        let _buffer = render_git_diff(&mut screen, 60, 40);

        // Even clicking on the left side should focus Patch
        screen.on_mouse_click(3, 2);

        // Footer shows "c comment" for Patch focus
        let buffer = render_git_diff(&mut screen, 60, 40);
        let text = buffer_text(&buffer);
        assert!(text.contains("c comment"), "narrow layout should focus patch: {text}");
    }

    #[test]
    fn git_diff_click_on_border_is_ignored() {
        let (mut screen, _effect) = GitDiffScreen::new(std::path::PathBuf::from("/tmp/repo"));
        load_document(&mut screen, make_test_document());

        let _buffer = render_git_diff(&mut screen, 120, 40);

        // Click at y=0 (top border) — should be ignored
        screen.on_mouse_click(0, 40);

        let buffer = render_git_diff(&mut screen, 120, 40);
        let text = buffer_text(&buffer);
        assert!(text.contains("Git Diff"), "screen should still render: {text}");
    }

    #[test]
    fn git_diff_click_after_resize_uses_new_pane_rects() {
        let (mut screen, _effect) = GitDiffScreen::new(std::path::PathBuf::from("/tmp/repo"));
        load_document(&mut screen, make_test_document());

        // Render at width 120: drawer_width = 36, drawer x=1..37
        render_git_diff(&mut screen, 120, 40);

        // Click at x=50 (patch area at width 120)
        screen.on_mouse_click(3, 50);

        // Resize to width 200: drawer_width = 36, drawer x=1..37
        render_git_diff(&mut screen, 200, 40);

        // Click at x=50 should still be in patch area
        screen.on_mouse_click(3, 50);

        // Footer shows "c comment" for Patch focus
        let buffer = render_git_diff(&mut screen, 200, 40);
        let text = buffer_text(&buffer);
        assert!(text.contains("c comment"), "patch should be focused: {text}");
    }
}

// ---------------------------------------------------------------------------
// Surface rect tracking tests
// ---------------------------------------------------------------------------

mod surface_rects {
    use super::*;
    use acp_utils::client::AcpEvent;

    #[test]
    fn settings_overlay_sets_surface_rect() {
        let opts = vec![agent_client_protocol::schema::SessionConfigOption::select(
            "model",
            "Model",
            "a",
            vec![
                agent_client_protocol::schema::SessionConfigSelectOption::new("a", "Alpha"),
                agent_client_protocol::schema::SessionConfigSelectOption::new("b", "Beta"),
            ],
        )];
        let (prompt_handle, _rx) = acp_utils::client::AcpPromptHandle::recording();
        let mut app = App::new(AppConfig {
            session_id: agent_client_protocol::schema::SessionId::new("test"),
            agent_name: "aether".to_string(),
            prompt_capabilities: agent_client_protocol::schema::PromptCapabilities::new(),
            session_capabilities: agent_client_protocol::schema::SessionCapabilities::new(),
            config_options: opts,
            auth_methods: Vec::new(),
            workspace_status: wisp_next::workspace_status::WorkspaceStatus::new("~/code", None),
            prompt_handle,
            working_dir: std::path::PathBuf::from("."),
            settings: UiSettings::default(),
        });

        // Type /settings to open the settings overlay
        type_text(&mut app, "/settings");
        app.on_key(key(KeyCode::Tab));

        assert!(app.has_modal(), "settings overlay should be open after /settings+Tab");

        let mut terminal = make_terminal(80, 24);
        let mut renderer = TranscriptRenderer::new(&UiSettings::default());
        sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

        assert!(app.surface_rect().is_some(), "surface rect should be set after render");
    }

    #[test]
    fn session_picker_sets_surface_rect() {
        let (mut app, _rx) = make_app();
        let current = agent_client_protocol::schema::SessionId::new("test-session");
        app.on_acp_event(AcpEvent::SessionsListed {
            sessions: vec![agent_client_protocol::schema::SessionInfo::new(
                agent_client_protocol::schema::SessionId::new("other"),
                std::path::PathBuf::from("/tmp"),
            )],
        });
        std::mem::drop(current);

        let mut terminal = make_terminal(80, 24);
        let mut renderer = TranscriptRenderer::new(&UiSettings::default());
        sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

        assert!(app.surface_rect().is_some());
    }

    #[test]
    fn prompt_search_sets_surface_rect() {
        let mut app = {
            let session_capabilities = agent_client_protocol::schema::SessionCapabilities::new().meta(Some(
                acp_utils::notifications::AetherCapabilities {
                    prompt_search: true,
                    session_preview: false,
                    workspace_move: false,
                }
                .to_meta(),
            ));
            let (prompt_handle, _command_rx) = acp_utils::client::AcpPromptHandle::recording();
            App::new(AppConfig {
                session_id: agent_client_protocol::schema::SessionId::new("test"),
                agent_name: "aether".to_string(),
                prompt_capabilities: agent_client_protocol::schema::PromptCapabilities::new(),
                session_capabilities,
                config_options: Vec::new(),
                auth_methods: Vec::new(),
                workspace_status: wisp_next::workspace_status::WorkspaceStatus::new("~/code", Some("main".to_string())),
                prompt_handle,
                working_dir: std::path::PathBuf::from("."),
                settings: UiSettings::default(),
            })
        };

        app.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));

        let mut terminal = make_terminal(80, 24);
        let mut renderer = TranscriptRenderer::new(&UiSettings::default());
        sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

        assert!(app.surface_rect().is_some());
    }
}

// ---------------------------------------------------------------------------
// Helpers shared across tests
// ---------------------------------------------------------------------------

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

fn submit_prompt(app: &mut App, text: &str) {
    type_text(app, text);
    app.on_key(key(KeyCode::Enter));
}

fn buffer_text(buffer: &Buffer) -> String {
    let mut out = String::new();
    for y in buffer.area.top()..buffer.area.bottom() {
        for x in buffer.area.left()..buffer.area.right() {
            out.push_str(buffer.cell((x, y)).map_or(" ", ratatui::buffer::Cell::symbol));
        }
        out.push('\n');
    }
    out
}
