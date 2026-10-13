use super::InitError;
use super::build_settings::{Preset, supported_providers};
use super::harness::HarnessIntegration;
use clap::ValueEnum;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind};
use futures::StreamExt;
use llm::catalog::Provider;
use ratatui::DefaultTerminal;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use std::io;

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
        None => match pick_one(&mut terminal, &mut events, "Choose a provider:", provider_options).await? {
            Some(provider) => provider,
            None => return Ok(None),
        },
    };

    let preset = match preset {
        Some(preset) => preset,
        None => match pick_one(&mut terminal, &mut events, "Choose a preset:", preset_options()).await? {
            Some(preset) => preset,
            None => return Ok(None),
        },
    };

    let harnesses = if preset == Preset::BatteriesIncluded && harnesses.is_empty() {
        match pick_harnesses(&mut terminal, &mut events).await? {
            Some(harnesses) => harnesses,
            None => return Ok(None),
        }
    } else {
        harnesses
    };

    Ok(Some((provider, preset, harnesses)))
}

struct WizardTerminal(DefaultTerminal);

impl WizardTerminal {
    fn enter() -> io::Result<Self> {
        ratatui::try_init().map(Self).inspect_err(|_| ratatui::restore())
    }

    fn draw(&mut self, lines: Vec<Line<'static>>) -> io::Result<()> {
        self.0.draw(|frame| frame.render_widget(Paragraph::new(lines), frame.area()))?;
        Ok(())
    }
}

impl Drop for WizardTerminal {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

struct SelectOption<T> {
    title: String,
    description: Option<String>,
    value: T,
}

struct Picker<T> {
    options: Vec<SelectOption<T>>,
    toggled: Vec<bool>,
    cursor: usize,
    multi: bool,
}

impl<T> Picker<T> {
    fn is_selected(&self, index: usize) -> bool {
        if self.multi { self.toggled[index] } else { index == self.cursor }
    }

    fn lines(&self, header: &str) -> Vec<Line<'static>> {
        let mut lines = header_lines(header);
        lines.extend(self.options.iter().enumerate().map(|(index, option)| {
            let marker = match (self.multi, self.is_selected(index)) {
                (true, true) => "[x] ",
                (true, false) => "[ ] ",
                (false, true) => "● ",
                (false, false) => "○ ",
            };
            let description =
                option.description.as_deref().map(|description| format!(" - {description}")).unwrap_or_default();
            let style = if self.multi && index == self.cursor {
                primary_style().add_modifier(Modifier::BOLD)
            } else if self.is_selected(index) {
                primary_style()
            } else {
                Style::default()
            };
            Line::styled(format!("{marker}{}{description}", option.title), style)
        }));

        let footer = if self.multi {
            "  ↑/↓ to move · Space to toggle · Enter to confirm · Esc to cancel"
        } else {
            "  ↑/↓ to move · Enter to confirm · Esc to cancel"
        };

        lines.extend(footer_lines(footer));
        lines
    }
}

async fn pick_harnesses(
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
    let picker = Picker { toggled: vec![true; options.len()], options, cursor: 0, multi: true };
    run_picker(terminal, events, "Load prompts, skills, and rules from other harness conventions?", picker).await
}

async fn pick_one<T>(
    terminal: &mut WizardTerminal,
    events: &mut EventStream,
    header: &str,
    options: Vec<(String, T)>,
) -> Result<Option<T>, InitError> {
    let options =
        options.into_iter().map(|(title, value)| SelectOption { title, description: None, value }).collect::<Vec<_>>();
    let picker = Picker { toggled: vec![false; options.len()], options, cursor: 0, multi: false };
    let chosen = run_picker(terminal, events, header, picker).await?;
    Ok(chosen.and_then(|values| values.into_iter().next()))
}

async fn run_picker<T>(
    terminal: &mut WizardTerminal,
    events: &mut EventStream,
    header: &str,
    mut picker: Picker<T>,
) -> Result<Option<Vec<T>>, InitError> {
    loop {
        terminal.draw(picker.lines(header)).map_err(InitError::Terminal)?;

        let Some(event) = next_event(events).await? else {
            return Ok(None);
        };
        let Event::Key(key) = event else {
            continue;
        };

        let len = picker.options.len();
        match key.code {
            KeyCode::Esc => return Ok(None),
            KeyCode::Enter => {
                let selected = (0..len).map(|index| picker.is_selected(index)).collect::<Vec<_>>();
                let chosen = picker
                    .options
                    .into_iter()
                    .zip(selected)
                    .filter_map(|(option, chosen)| chosen.then_some(option.value));
                return Ok(Some(chosen.collect()));
            }
            KeyCode::Char(' ') if picker.multi && len > 0 => {
                picker.toggled[picker.cursor] = !picker.toggled[picker.cursor];
            }
            KeyCode::Up if len > 0 => picker.cursor = (picker.cursor + len - 1) % len,
            KeyCode::Down if len > 0 => picker.cursor = (picker.cursor + 1) % len,
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

fn header_lines(header: &str) -> Vec<Line<'static>> {
    vec![Line::styled(header.to_string(), primary_style()), Line::default()]
}

fn footer_lines(footer: &str) -> [Line<'static>; 2] {
    [Line::default(), Line::styled(footer.to_string(), Style::default().fg(Color::DarkGray))]
}

fn primary_style() -> Style {
    Style::default().fg(Color::Rgb(255, 215, 0))
}
