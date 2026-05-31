use super::InitError;
use super::build_settings::{Preset, supported_providers};
use llm::catalog::Provider;
use std::io;
use tui::{
    Component, CrosstermEvent, Event, Frame, KeyCode, Line, MouseCapture, RadioSelect, SelectOption, Style,
    TerminalConfig, TerminalRuntime, Theme, ViewContext, terminal_size,
};

pub async fn run_wizard(
    provider: Option<Provider>,
    preset: Option<Preset>,
) -> Result<Option<(Provider, Preset)>, InitError> {
    if let (Some(p), Some(t)) = (provider, preset) {
        return Ok(Some((p, t)));
    }

    let mut terminal = TerminalRuntime::new(
        io::stdout(),
        Theme::default(),
        terminal_size().unwrap_or((80, 24)),
        TerminalConfig { bracketed_paste: false, mouse_capture: MouseCapture::Disabled },
    )
    .map_err(InitError::Terminal)?;

    let provider_options = supported_providers().map(|p| (format_provider_title(p), p)).collect();
    let provider = match provider {
        Some(p) => p,
        None => match run_select(&mut terminal, "Choose a provider:", provider_options).await? {
            Some(p) => p,
            None => return Ok(None),
        },
    };

    let resolved_preset = match preset {
        Some(t) => t,
        None => match run_select(&mut terminal, "Choose a preset:", preset_options()).await? {
            Some(t) => t,
            None => return Ok(None),
        },
    };

    Ok(Some((provider, resolved_preset)))
}

fn preset_options() -> Vec<(String, Preset)> {
    vec![
        ("Minimal — one agent with bash + skills only".to_string(), Preset::Minimal),
        (
            "Batteries-included — Plan + Build + Explore agents wired to coding/skills/subagents".to_string(),
            Preset::BatteriesIncluded,
        ),
    ]
}

fn format_provider_title(provider: Provider) -> String {
    let marker = match provider.required_env_var().filter(|v| std::env::var(v).is_err()) {
        None => "\u{2713} ready".to_string(),
        Some(var) => format!("\u{26a0} set ${var}"),
    };
    format!("{:<12} {marker}", provider.display_name())
}

async fn run_select<T: Clone>(
    terminal: &mut TerminalRuntime<io::Stdout>,
    prompt: &str,
    options: Vec<(String, T)>,
) -> Result<Option<T>, InitError> {
    let select_options = options
        .iter()
        .enumerate()
        .map(|(i, (title, _))| SelectOption { value: i.to_string(), title: title.clone(), description: None })
        .collect();

    let mut select = RadioSelect::new(select_options, 0);
    terminal.render_frame(|ctx| render(&select, prompt, ctx)).map_err(InitError::Terminal)?;

    loop {
        let Some(event) = terminal.next_event().await else {
            return Ok(None);
        };

        if let CrosstermEvent::Resize(c, r) = &event {
            terminal.on_resize((*c, *r));
        }

        let Ok(tui_event) = Event::try_from(event) else { continue };
        if let Event::Key(key) = &tui_event {
            match key.code {
                KeyCode::Esc => {
                    let _ = terminal.clear_screen();
                    return Ok(None);
                }

                KeyCode::Enter => {
                    let _ = terminal.clear_screen();
                    return Ok(options.get(select.selected).map(|(_, v)| v.clone()));
                }
                _ => {}
            }
        }

        select.on_event(&tui_event).await;
        terminal.render_frame(|ctx| render(&select, prompt, ctx)).map_err(InitError::Terminal)?;
    }
}

fn render(select: &RadioSelect, header: &str, ctx: &ViewContext) -> Frame {
    let mut lines = vec![Line::with_style(header.to_string(), Style::fg(ctx.theme.primary())), Line::default()];
    lines.extend(select.render_field(ctx, true));
    lines.push(Line::default());
    lines.push(Line::styled(
        "  \u{2191}/\u{2193} to move \u{00b7} Enter to confirm \u{00b7} Esc to cancel",
        ctx.theme.muted(),
    ));
    Frame::new(lines)
}
