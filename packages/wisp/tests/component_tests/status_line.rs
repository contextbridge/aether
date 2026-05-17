use acp_utils::config_option_id::ConfigOptionId;
use agent_client_protocol::schema::{self as acp, SessionConfigOption, SessionConfigOptionCategory};
use tui::ViewContext;
use tui::testing::render_lines;
use wisp::components::status_line::{ContextUsageDisplay, StatusLine};
use wisp::settings::DEFAULT_CONTENT_PADDING;
use wisp::workspace_status::WorkspaceStatus;

fn mode_option(value: impl Into<String>, name: impl Into<String>) -> SessionConfigOption {
    let value = value.into();
    let name = name.into();
    SessionConfigOption::select("mode", "Mode", value.clone(), vec![acp::SessionConfigSelectOption::new(value, name)])
        .category(SessionConfigOptionCategory::Mode)
}

fn model_option(value: impl Into<String>, name: impl Into<String>) -> SessionConfigOption {
    let value = value.into();
    let name = name.into();
    SessionConfigOption::select("model", "Model", value.clone(), vec![acp::SessionConfigSelectOption::new(value, name)])
        .category(SessionConfigOptionCategory::Model)
}

fn reasoning_option(value: impl Into<String>) -> SessionConfigOption {
    let value = value.into();
    SessionConfigOption::select(
        ConfigOptionId::ReasoningEffort.as_str(),
        "Reasoning",
        value,
        vec![
            acp::SessionConfigSelectOption::new("none", "None"),
            acp::SessionConfigSelectOption::new("low", "Low"),
            acp::SessionConfigSelectOption::new("medium", "Medium"),
            acp::SessionConfigSelectOption::new("high", "High"),
        ],
    )
}

struct StatusBuilder<'a> {
    name: &'a str,
    options: Vec<SessionConfigOption>,
    ctx_usage: Option<ContextUsageDisplay>,
    waiting: bool,
    unhealthy: usize,
    width: u16,
    workspace_status: WorkspaceStatus,
}

impl<'a> StatusBuilder<'a> {
    fn new(name: &'a str) -> Self {
        Self {
            name,
            options: vec![],
            ctx_usage: None,
            waiting: false,
            unhealthy: 0,
            width: 80,
            workspace_status: WorkspaceStatus::new("~/code/aether-2", Some("main".to_string())),
        }
    }

    fn model(mut self, m: &str) -> Self {
        self.options.push(model_option(m, m));
        self
    }
    fn mode(mut self, value: &str, display: &str) -> Self {
        self.options.push(mode_option(value, display));
        self
    }
    fn reasoning(mut self, v: &str) -> Self {
        self.options.push(reasoning_option(v));
        self
    }
    fn ctx_usage(mut self, used: u32, limit: u32) -> Self {
        self.ctx_usage = Some(ContextUsageDisplay::new(used, limit));
        self
    }
    fn waiting(mut self) -> Self {
        self.waiting = true;
        self
    }
    fn unhealthy(mut self, n: usize) -> Self {
        self.unhealthy = n;
        self
    }
    fn width(mut self, w: u16) -> Self {
        self.width = w;
        self
    }
    fn workspace(mut self, dir: &str, git_ref: Option<&str>) -> Self {
        self.workspace_status = WorkspaceStatus::new(dir, git_ref.map(str::to_string));
        self
    }

    fn render(&self) -> (String, tui::testing::TestTerminal) {
        let status = StatusLine {
            workspace_status: &self.workspace_status,
            agent_name: self.name,
            config_options: &self.options,
            context_usage: self.ctx_usage,
            waiting_for_response: self.waiting,
            unhealthy_server_count: self.unhealthy,
            content_padding: DEFAULT_CONTENT_PADDING,
            exit_confirmation_active: false,
        };
        let ctx = ViewContext::new((self.width, 24));
        let frame = status.render(&ctx);
        let term = render_lines(frame.lines(), self.width, 24);
        let line = term.get_lines()[0].clone();
        (line, term)
    }

    fn line(&self) -> String {
        self.render().0
    }
}

#[test]
fn renders_workspace_on_left_and_existing_status_on_right() {
    let line = StatusBuilder::new("Aether")
        .mode("planner", "Planner")
        .model("Codex: GPT-5.5")
        .reasoning("medium")
        .ctx_usage(164_000, 272_000)
        .width(140)
        .line();

    let workspace_at = line.find("~/code/aether-2 · main").unwrap();
    let agent_at = line.find("Aether").unwrap();
    assert!(workspace_at < agent_at);
    assert!(line.contains("Planner"));
    assert!(line.contains("Codex: GPT-5.5"));
    assert!(line.contains("medium [■■·]"));
    assert!(line.contains("ctx"));
}

#[test]
fn renders_workspace_without_git_ref() {
    let line = StatusBuilder::new("aether").workspace("~/scratch", None).line();
    assert!(line.contains("~/scratch"));
    assert!(!line.contains("main"));
}

#[test]
fn renders_workspace_with_expected_colors() {
    let ctx = ViewContext::new((80, 24));
    let (_, term) = StatusBuilder::new("wisp").render();

    assert_eq!(term.style_of_text(0, "~/code/aether-2").unwrap().fg, Some(ctx.theme.secondary()));
    assert_eq!(term.style_of_text(0, " · ").unwrap().fg, Some(ctx.theme.text_secondary()));
    assert_eq!(term.style_of_text(0, "main").unwrap().fg, Some(ctx.theme.success()));
}

#[test]
fn keeps_status_area_to_one_line_when_narrow() {
    let workspace_status = WorkspaceStatus::new(
        "~/very/long/path/that/would/wrap/without/truncation",
        Some("feature/very-long-branch".to_string()),
    );
    let options = vec![model_option("model", "very-long-model-name")];
    let status = StatusLine {
        workspace_status: &workspace_status,
        agent_name: "agent-name",
        config_options: &options,
        context_usage: Some(ContextUsageDisplay::new(100_000, 200_000)),
        waiting_for_response: false,
        unhealthy_server_count: 0,
        content_padding: DEFAULT_CONTENT_PADDING,
        exit_confirmation_active: false,
    };
    let ctx = ViewContext::new((24, 24));

    assert_eq!(status.render(&ctx).lines().len(), 1);
}

#[test]
fn exit_confirmation_keeps_workspace_and_moves_warning_right() {
    let workspace_status = WorkspaceStatus::new("~/code/aether-2", Some("main".to_string()));
    let status = StatusLine {
        workspace_status: &workspace_status,
        agent_name: "agent-name",
        config_options: &[],
        context_usage: None,
        waiting_for_response: false,
        unhealthy_server_count: 0,
        content_padding: DEFAULT_CONTENT_PADDING,
        exit_confirmation_active: true,
    };
    let ctx = ViewContext::new((100, 24));
    let text = status.render(&ctx).lines()[0].plain_text();

    assert!(text.contains("~/code/aether-2 · main"));
    assert!(text.contains("Ctrl-C again to exit"));
    assert!(!text.contains("agent-name"));
}

#[test]
fn renders_agent_name() {
    let line = StatusBuilder::new("test-agent").line();
    assert!(line.contains("test-agent"));
}

#[test]
fn renders_model_display() {
    let line = StatusBuilder::new("aether-acp").model("gpt-4o").line();
    assert!(line.contains("aether-acp"));
    assert!(line.contains("gpt-4o"));
}

#[test]
fn renders_without_model_when_none() {
    let line = StatusBuilder::new("aether-acp").workspace("~/code/aether-2", None).line();
    assert!(line.contains("aether-acp"));
    assert!(!line.contains("·"), "no separator when no model");
}

#[test]
fn renders_context_usage() {
    let line = StatusBuilder::new("aether").model("gpt-4o").ctx_usage(150_000, 200_000).line();
    assert!(line.contains("ctx") && line.contains("150k / 200k"));

    // Works when waiting
    let line = StatusBuilder::new("aether").model("gpt-4o").ctx_usage(100_000, 200_000).waiting().line();
    assert!(line.contains("ctx") && line.contains("100k / 200k"));
}

#[test]
fn renders_no_context_segment_when_usage_unknown() {
    let line = StatusBuilder::new("aether").model("gpt-4o").line();
    assert!(!line.contains("ctx"), "context segment should be hidden when no usage data; got: {line}");
}

#[test]
fn renders_agent_name_when_waiting_without_model() {
    let line = StatusBuilder::new("aether").waiting().line();
    assert!(line.contains("aether"));
}

#[test]
fn renders_unhealthy_servers() {
    let line = StatusBuilder::new("aether").model("gpt-4o").unhealthy(1).line();
    assert!(line.contains("1 server needs auth"));

    let line = StatusBuilder::new("aether").unhealthy(3).line();
    assert!(line.contains("3 servers unhealthy"));

    let line = StatusBuilder::new("aether").line();
    assert!(!line.contains("server"));
}

#[test]
fn renders_both_context_and_unhealthy() {
    let line = StatusBuilder::new("aether").ctx_usage(100_000, 200_000).unhealthy(2).width(120).line();
    assert!(line.contains("ctx") && line.contains("100k / 200k"));
    assert!(line.contains("2 servers unhealthy"));
}

#[test]
fn renders_agent_mode_model_in_order() {
    let line = StatusBuilder::new("wisp").mode("planner", "Planner").model("gpt-4o").line();
    let agent_at = line.find("wisp").unwrap();
    let mode_at = line.find("Planner").unwrap();
    let llm_model_at = line.find("gpt-4o").unwrap();
    assert!(agent_at < mode_at && mode_at < llm_model_at);
}

#[test]
fn renders_elements_with_correct_colors() {
    let ctx = ViewContext::new((80, 24));
    let (_, term) = StatusBuilder::new("wisp").mode("planner", "Planner").model("gpt-4o").render();

    assert_eq!(term.style_of_text(0, "wisp").unwrap().fg, Some(ctx.theme.info()));
    assert_eq!(term.style_of_text(0, "Planner").unwrap().fg, Some(ctx.theme.secondary()));
    assert_eq!(term.style_of_text(0, "gpt-4o").unwrap().fg, Some(ctx.theme.success()));

    // All three should be distinct
    let colors: Vec<_> = ["wisp", "Planner", "gpt-4o"].iter().map(|s| term.style_of_text(0, s).map(|s| s.fg)).collect();
    assert_ne!(colors[0], colors[1]);
    assert_ne!(colors[1], colors[2]);
    assert_ne!(colors[0], colors[2]);
}

#[test]
fn renders_reasoning_bar() {
    // Medium effort
    let line = StatusBuilder::new("wisp").model("gpt-4o").reasoning("medium").line();
    assert!(line.contains("medium [■■·]"));
    assert!(line.find("gpt-4o").unwrap() < line.find("medium").unwrap());
    assert!(!line.contains("reasoning"));

    // None effort shows empty bar
    let line = StatusBuilder::new("wisp").model("gpt-4o").reasoning("none").line();
    assert!(line.contains("none [···]"));

    // No model = no reasoning bar even with reasoning set
    let line = StatusBuilder::new("wisp").reasoning("high").line();
    assert!(!line.contains("reasoning"));
}

#[test]
fn renders_reasoning_bar_high_with_success_color() {
    let ctx = ViewContext::new((80, 24));
    let (_, term) = StatusBuilder::new("wisp").model("gpt-4o").reasoning("high").render();
    assert_eq!(term.style_of_text(0, "■").unwrap().fg, Some(ctx.theme.success()));
}
