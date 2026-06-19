use std::time::Duration;
#[allow(clippy::wildcard_imports)]
use tui::*;

#[derive(Debug, Clone)]
struct Language {
    name: String,
    ext: String,
}

impl Searchable for Language {
    fn search_text(&self) -> String {
        format!("{} {}", self.name, self.ext)
    }
}

struct ComboboxStory {
    combobox: Combobox<Language>,
}

impl Component for ComboboxStory {
    type Message = ();

    async fn on_event(&mut self, event: &Event) -> Option<Vec<()>> {
        self.combobox.handle_picker_event(event).map(|_| vec![])
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        let lines = self.combobox.render_items(ctx, |lang, selected, inner| {
            let text = format!("{} (.{})", lang.name, lang.ext);
            if selected {
                let mut line = Line::with_style(text, inner.theme.selected_row_style());
                line.extend_bg_to_width(inner.size.width as usize);
                line
            } else {
                Line::new(text)
            }
        });
        Frame::new(lines)
    }
}

enum TuiStory {
    TextField(TextField),
    Checkbox(Checkbox),
    Spinner(Spinner),
    NumberField(NumberField),
    RadioSelect(RadioSelect),
    MultiSelect(MultiSelect),
    Combobox(ComboboxStory),
}

impl Component for TuiStory {
    type Message = ();

    async fn on_event(&mut self, event: &Event) -> Option<Vec<()>> {
        match self {
            Self::TextField(c) => c.on_event(event).await.map(|_| vec![]),
            Self::Checkbox(c) => c.on_event(event).await.map(|_| vec![]),
            Self::Spinner(c) => c.on_event(event).await,
            Self::NumberField(c) => c.on_event(event).await.map(|_| vec![]),
            Self::RadioSelect(c) => c.on_event(event).await.map(|_| vec![]),
            Self::MultiSelect(c) => c.on_event(event).await.map(|_| vec![]),
            Self::Combobox(c) => c.on_event(event).await,
        }
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        match self {
            Self::TextField(c) => c.render(ctx),
            Self::Checkbox(c) => c.render(ctx),
            Self::Spinner(c) => c.render(ctx),
            Self::NumberField(c) => c.render(ctx),
            Self::RadioSelect(c) => c.render(ctx),
            Self::MultiSelect(c) => c.render(ctx),
            Self::Combobox(c) => c.render(ctx),
        }
    }
}

fn sample_options() -> Vec<SelectOption> {
    vec![
        SelectOption { value: "rust".into(), title: "Rust".into(), description: Some("Systems programming".into()) },
        SelectOption { value: "go".into(), title: "Go".into(), description: Some("Cloud infrastructure".into()) },
        SelectOption { value: "python".into(), title: "Python".into(), description: Some("Data science & ML".into()) },
    ]
}

fn sample_languages() -> Vec<Language> {
    vec![
        Language { name: "Rust".into(), ext: "rs".into() },
        Language { name: "Python".into(), ext: "py".into() },
        Language { name: "TypeScript".into(), ext: "ts".into() },
        Language { name: "Go".into(), ext: "go".into() },
        Language { name: "Ruby".into(), ext: "rb".into() },
        Language { name: "Java".into(), ext: "java".into() },
        Language { name: "C++".into(), ext: "cpp".into() },
        Language { name: "Haskell".into(), ext: "hs".into() },
    ]
}

fn stories() -> Vec<(String, TuiStory)> {
    vec![
        ("TextField".into(), TuiStory::TextField(TextField::new("Hello, gallery!".into()))),
        ("Checkbox".into(), TuiStory::Checkbox(Checkbox::new(false))),
        (
            "Spinner".into(),
            TuiStory::Spinner({
                let mut s = Spinner::braille();
                s.visible = true;
                s
            }),
        ),
        ("NumberField".into(), TuiStory::NumberField(NumberField::new(String::new(), false))),
        ("RadioSelect".into(), TuiStory::RadioSelect(RadioSelect::new(sample_options(), 0))),
        ("MultiSelect".into(), TuiStory::MultiSelect(MultiSelect::new(sample_options(), vec![false, false, false]))),
        ("Combobox".into(), TuiStory::Combobox(ComboboxStory { combobox: Combobox::new(sample_languages()) })),
    ]
}

#[tokio::main]
async fn main() {
    let mut gallery = Gallery::new(stories());
    let size = terminal_size().unwrap_or((80, 24));
    let mut terminal = TerminalRuntime::new(
        std::io::stdout(),
        Theme::default(),
        size,
        TerminalConfig { bracketed_paste: true, mouse_capture: MouseCapture::Disabled },
    )
    .unwrap();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    terminal.render_frame(|ctx| gallery.render(ctx)).unwrap();

    loop {
        tokio::select! {
            Some(raw) = terminal.next_event() => {
                if let CrosstermEvent::Resize(cols, rows) = &raw {
                    terminal.on_resize((*cols, *rows));
                }
                if let Ok(event) = Event::try_from(raw) {
                    if let Some(msgs) = gallery.on_event(&event).await
                        && msgs.iter().any(|m| matches!(m, GalleryMessage::Quit))
                    {
                        return;
                    }
                    terminal.render_frame(|ctx| gallery.render(ctx)).unwrap();
                }
            }
            _ = tick.tick() => {
                gallery.on_event(&Event::Tick).await;
                terminal.render_frame(|ctx| gallery.render(ctx)).unwrap();
            }
        }
    }
}
