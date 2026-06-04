use super::InitError;
use super::build_settings::{Preset, supported_providers};
use super::harness::HarnessIntegration;
use clap::ValueEnum;
use llm::catalog::Provider;
use std::io;
use tui::{
    Component, CrosstermEvent, Event, Frame, KeyCode, Line, MouseCapture, MultiSelect, RadioSelect, SelectOption,
    Style, TerminalConfig, TerminalRuntime, Theme, ViewContext, terminal_size,
};

pub async fn run_wizard(
    provider: Option<Provider>,
    preset: Option<Preset>,
    harnesses: Vec<HarnessIntegration>,
) -> Result<Option<(Provider, Preset, Vec<HarnessIntegration>)>, InitError> {
    if let (Some(p), Some(t)) = (provider, preset) {
        return Ok(Some((p, t, harnesses)));
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

    let selected_harnesses = if resolved_preset == Preset::BatteriesIncluded && harnesses.is_empty() {
        match run_multi_select(&mut terminal).await? {
            Some(h) => h,
            None => return Ok(None),
        }
    } else {
        harnesses
    };

    Ok(Some((provider, resolved_preset, selected_harnesses)))
}

fn harness_options() -> Vec<(SelectOption, HarnessIntegration)> {
    HarnessIntegration::value_variants()
        .iter()
        .map(|h| {
            let value = h.to_possible_value().map(|v| v.get_name().to_string()).unwrap_or_default();
            (SelectOption { value, title: h.title(), description: Some(h.description()) }, *h)
        })
        .collect()
}

async fn run_multi_select(
    terminal: &mut TerminalRuntime<io::Stdout>,
) -> Result<Option<Vec<HarnessIntegration>>, InitError> {
    let options = harness_options();
    let select_options: Vec<SelectOption> = options.iter().map(|(opt, _)| opt.clone()).collect();
    let all_selected = vec![true; options.len()];
    let mut select = MultiSelect::new(select_options, all_selected);
    let header = "Load prompts, skills, and rules from other harness conventions?";
    let footer =
        "  \u{2191}/\u{2193} to move \u{00b7} Space to toggle \u{00b7} Enter to confirm \u{00b7} Esc to cancel";

    let render = |s: &MultiSelect, ctx: &ViewContext| render_select(s, header, footer, ctx);
    if !run_event_loop(terminal, &mut select, render).await? {
        return Ok(None);
    }

    let selected: Vec<HarnessIntegration> =
        select.selected.iter().enumerate().filter(|(_, s)| **s).map(|(i, _)| options[i].1).collect();
    Ok(Some(selected))
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
    let footer = "  \u{2191}/\u{2193} to move \u{00b7} Enter to confirm \u{00b7} Esc to cancel";

    let render = |s: &RadioSelect, ctx: &ViewContext| render_select(s, prompt, footer, ctx);
    if !run_event_loop(terminal, &mut select, render).await? {
        return Ok(None);
    }

    Ok(options.get(select.selected).map(|(_, v)| v.clone()))
}

async fn run_event_loop<W: Component>(
    terminal: &mut TerminalRuntime<io::Stdout>,
    widget: &mut W,
    render_fn: impl Fn(&W, &ViewContext) -> Frame,
) -> Result<bool, InitError> {
    draw(terminal, widget, &render_fn)?;

    loop {
        let Some(event) = terminal.next_event().await else {
            return Ok(false);
        };

        if let CrosstermEvent::Resize(c, r) = &event {
            terminal.on_resize((*c, *r));
        }

        let Ok(tui_event) = Event::try_from(event) else { continue };
        if let Event::Key(key) = &tui_event {
            match key.code {
                KeyCode::Esc => {
                    let _ = terminal.clear_screen();
                    return Ok(false);
                }
                KeyCode::Enter => {
                    let _ = terminal.clear_screen();
                    return Ok(true);
                }
                _ => {}
            }
        }

        widget.on_event(&tui_event).await;
        draw(terminal, widget, &render_fn)?;
    }
}

fn draw<T>(
    terminal: &mut TerminalRuntime<io::Stdout>,
    widget: &T,
    render_fn: &impl Fn(&T, &ViewContext) -> Frame,
) -> Result<(), InitError> {
    terminal.render_frame(|ctx| render_fn(widget, ctx)).map_err(InitError::Terminal)
}

fn render_select<T>(widget: &T, header: &str, footer: &str, ctx: &ViewContext) -> Frame
where
    T: SelectField,
{
    let mut lines = vec![Line::with_style(header.to_string(), Style::fg(ctx.theme.primary())), Line::default()];
    lines.extend(widget.field_lines(ctx));
    lines.push(Line::default());
    lines.push(Line::styled(footer.to_string(), ctx.theme.muted()));
    Frame::new(lines)
}

trait SelectField {
    fn field_lines(&self, ctx: &ViewContext) -> Vec<Line>;
}

impl SelectField for RadioSelect {
    fn field_lines(&self, ctx: &ViewContext) -> Vec<Line> {
        RadioSelect::render_field(self, ctx, true)
    }
}

impl SelectField for MultiSelect {
    fn field_lines(&self, ctx: &ViewContext) -> Vec<Line> {
        MultiSelect::render_field(self, ctx, true)
    }
}
