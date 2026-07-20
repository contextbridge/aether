use super::InitError;
use super::build_settings::{Preset, supported_providers};
use super::harness::HarnessIntegration;
use clap::ValueEnum;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    Event, EventStream, KeyCode, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use futures::StreamExt;
use llm::catalog::Provider;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use std::io::{self, Stdout};

pub async fn run_wizard(
    provider: Option<Provider>,
    preset: Option<Preset>,
    harnesses: Vec<HarnessIntegration>,
) -> Result<Option<(Provider, Preset, Vec<HarnessIntegration>)>, InitError> {
    if let (Some(provider), Some(preset)) = (provider, preset) {
        return Ok(Some((provider, preset, harnesses)));
    }

    let mut terminal = WizardTerminal::enter().map_err(InitError::Terminal)?;
    let mut events = EventStream::new();

    let provider_options = supported_providers().map(|provider| (format_provider_title(provider), provider)).collect();
    let provider = match provider {
        Some(provider) => provider,
        None => match run_select(&mut terminal, &mut events, "Choose a provider:", provider_options).await? {
            Some(provider) => provider,
            None => return Ok(None),
        },
    };

    let preset = match preset {
        Some(preset) => preset,
        None => match run_select(&mut terminal, &mut events, "Choose a preset:", preset_options()).await? {
            Some(preset) => preset,
            None => return Ok(None),
        },
    };

    let harnesses = if preset == Preset::BatteriesIncluded && harnesses.is_empty() {
        match run_multi_select(&mut terminal, &mut events).await? {
            Some(harnesses) => harnesses,
            None => return Ok(None),
        }
    } else {
        harnesses
    };

    Ok(Some((provider, preset, harnesses)))
}

struct WizardTerminal {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    keyboard_enhancement: bool,
}

impl WizardTerminal {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;

        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(error);
        }

        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES;
        let keyboard_enhancement = if execute!(stdout, PushKeyboardEnhancementFlags(flags)).is_ok() {
            true
        } else {
            execute!(stdout, PopKeyboardEnhancementFlags).is_err()
        };

        match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => Ok(Self { terminal, keyboard_enhancement }),
            Err(error) => {
                let mut stdout = io::stdout();
                if keyboard_enhancement {
                    let _ = execute!(stdout, PopKeyboardEnhancementFlags);
                }
                let _ = execute!(stdout, Show, LeaveAlternateScreen);
                let _ = disable_raw_mode();
                Err(error)
            }
        }
    }

    fn draw(&mut self, lines: Vec<Line<'static>>) -> io::Result<()> {
        self.terminal.draw(|frame| {
            frame.render_widget(Paragraph::new(lines), frame.area());
        })?;
        Ok(())
    }
}

impl Drop for WizardTerminal {
    fn drop(&mut self) {
        if self.keyboard_enhancement {
            let _ = execute!(self.terminal.backend_mut(), PopKeyboardEnhancementFlags);
        }
        let _ = execute!(self.terminal.backend_mut(), Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

struct SelectOption<T> {
    title: String,
    description: Option<String>,
    value: T,
}

struct MultiSelect<T> {
    options: Vec<SelectOption<T>>,
    selected: Vec<bool>,
    cursor: usize,
}

async fn run_multi_select(
    terminal: &mut WizardTerminal,
    events: &mut EventStream,
) -> Result<Option<Vec<HarnessIntegration>>, InitError> {
    let options = HarnessIntegration::value_variants()
        .iter()
        .map(|harness| SelectOption {
            title: harness.title(),
            description: Some(harness.description()),
            value: *harness,
        })
        .collect::<Vec<_>>();
    let mut select = MultiSelect { selected: vec![true; options.len()], options, cursor: 0 };
    let header = "Load prompts, skills, and rules from other harness conventions?";
    let footer = "  ↑/↓ to move · Space to toggle · Enter to confirm · Esc to cancel";

    loop {
        terminal.draw(render_multi_select(&select, header, footer)).map_err(InitError::Terminal)?;

        let Some(event) = next_event(events).await? else {
            return Ok(None);
        };
        let Event::Key(key) = event else {
            continue;
        };

        match key.code {
            KeyCode::Esc => return Ok(None),
            KeyCode::Enter => {
                let selected = select
                    .options
                    .into_iter()
                    .zip(select.selected)
                    .filter_map(|(option, selected)| selected.then_some(option.value))
                    .collect();
                return Ok(Some(selected));
            }
            KeyCode::Char(' ') if !select.options.is_empty() => {
                select.selected[select.cursor] = !select.selected[select.cursor];
            }
            KeyCode::Up if !select.options.is_empty() => {
                select.cursor = (select.cursor + select.options.len() - 1) % select.options.len();
            }
            KeyCode::Down if !select.options.is_empty() => {
                select.cursor = (select.cursor + 1) % select.options.len();
            }
            _ => {}
        }
    }
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
    let marker = match provider.required_env_var().filter(|variable| std::env::var(variable).is_err()) {
        None => "✓ ready".to_string(),
        Some(variable) => format!("⚠ set ${variable}"),
    };
    format!("{:<12} {marker}", provider.display_name())
}

async fn run_select<T>(
    terminal: &mut WizardTerminal,
    events: &mut EventStream,
    prompt: &str,
    options: Vec<(String, T)>,
) -> Result<Option<T>, InitError> {
    let options =
        options.into_iter().map(|(title, value)| SelectOption { title, description: None, value }).collect::<Vec<_>>();
    let mut selected = 0;
    let footer = "  ↑/↓ to move · Enter to confirm · Esc to cancel";

    loop {
        terminal.draw(render_select(&options, selected, prompt, footer)).map_err(InitError::Terminal)?;

        let Some(event) = next_event(events).await? else {
            return Ok(None);
        };
        let Event::Key(key) = event else {
            continue;
        };

        match key.code {
            KeyCode::Esc => return Ok(None),
            KeyCode::Enter => return Ok(options.into_iter().nth(selected).map(|option| option.value)),
            KeyCode::Up if !options.is_empty() => {
                selected = (selected + options.len() - 1) % options.len();
            }
            KeyCode::Down if !options.is_empty() => {
                selected = (selected + 1) % options.len();
            }
            _ => {}
        }
    }
}

async fn next_event(events: &mut EventStream) -> Result<Option<Event>, InitError> {
    loop {
        let Some(event) = events.next().await else {
            return Ok(None);
        };
        let event = event.map_err(InitError::Terminal)?;
        if matches!(&event, Event::Key(key) if matches!(key.kind, KeyEventKind::Release)) {
            continue;
        }
        return Ok(Some(event));
    }
}

fn render_select<T>(options: &[SelectOption<T>], selected: usize, header: &str, footer: &str) -> Vec<Line<'static>> {
    let mut lines = header_lines(header);
    lines.extend(options.iter().enumerate().map(|(index, option)| {
        let marker = if index == selected { "● " } else { "○ " };
        let style = if index == selected { primary_style() } else { Style::default() };
        Line::styled(format!("{marker}{}", option.title), style)
    }));
    lines.extend(footer_lines(footer));
    lines
}

fn render_multi_select<T>(select: &MultiSelect<T>, header: &str, footer: &str) -> Vec<Line<'static>> {
    let mut lines = header_lines(header);
    lines.extend(select.options.iter().enumerate().map(|(index, option)| {
        let marker = if select.selected[index] { "[x] " } else { "[ ] " };
        let description =
            option.description.as_deref().map(|description| format!(" - {description}")).unwrap_or_default();
        let style = if index == select.cursor {
            primary_style().add_modifier(Modifier::BOLD)
        } else if select.selected[index] {
            primary_style()
        } else {
            Style::default()
        };
        Line::styled(format!("{marker}{}{description}", option.title), style)
    }));
    lines.extend(footer_lines(footer));
    lines
}

fn header_lines(header: &str) -> Vec<Line<'static>> {
    vec![Line::styled(header.to_string(), primary_style()), Line::default()]
}

fn footer_lines(footer: &str) -> [Line<'static>; 2] {
    [Line::default(), Line::styled(footer.to_string(), Style::default().fg(Color::DarkGray))]
}

fn primary_style() -> Style {
    Style::default().fg(Color::Rgb(255, 215, 0))
}
