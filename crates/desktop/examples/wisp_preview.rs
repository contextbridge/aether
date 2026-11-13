use aether_desktop::ui::{
    ActiveReviewTheme, ButtonVariant, ControlSize, Icon, IconName, UI_FONT, UiStyles, WispMotion, WispState, button,
    wisp,
};
use clankerdiff_ui_core::ReviewTheme;
use clap::Parser;
use rouge::core::{Color, Styled, View, div, scroll, text, view};
use rouge::reactive::{Runtime, signal, sleep, spawn};
use rouge::{App, WindowOptions};
use rouge_widgets::bundled_fonts;
use rouge_widgets::testing::component_test;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::runtime::Runtime as TokioRuntime;

#[derive(Parser)]
#[command(about = "Preview the actual desktop shader, capture a deterministic frame, or measure rendering work")]
struct Options {
    #[arg(long)]
    capture: Option<PathBuf>,
    #[arg(long)]
    benchmark: bool,
    #[arg(long)]
    reduced: bool,
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
}

fn main() {
    let options = Options::parse();
    if options.capture.is_some() || options.benchmark {
        let _entered = Runtime::new().enter();
        let mut ui = component_test().size(960.0, 840.0).scale(options.scale).build(move || preview(options.reduced));
        ui.run(1.3);
        if let Some(path) = options.capture {
            let image = ui.render().expect("render the wisp gallery");
            fs::write(&path, image.to_png().expect("encode the gallery")).expect("write the gallery");
            println!("Captured {}", path.display());
        }
        if options.benchmark {
            let mut work = ui.draw().expect("warm the pipelines");
            ui.wait().expect("finish the warmup");
            let before = ui.stats();
            let mut samples = Vec::new();
            for _ in 0..120 {
                let start = Instant::now();
                ui.advance(1.0 / 60.0);
                work = ui.draw().expect("draw the benchmark frame");
                ui.wait().expect("complete the benchmark frame");
                samples.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            let after = ui.stats();
            samples.sort_by(f64::total_cmp);
            println!(
                "120 synchronized headless frames; wall time includes CPU work, GPU completion, and driver overhead"
            );
            println!(
                "mean {:.2}ms, p95 {:.2}ms; scale {}x",
                samples.iter().sum::<f64>() / 120.0,
                samples[113],
                options.scale
            );
            println!("layout delta {}, paint delta {}", after.layouts - before.layouts, after.paints - before.paints);
            println!("last-frame work: {work:?}");
        }
        return;
    }
    let runtime = TokioRuntime::new().expect("start the Tokio runtime");
    App::new()
        .fonts(bundled_fonts())
        .tokio(runtime.handle().clone())
        .window(WindowOptions::default().title("Aether · Activity motion").size(960.0, 840.0), move || {
            preview(options.reduced)
        })
        .run()
        .expect("open the wisp preview");
}

fn preview(reduced: bool) -> impl View {
    let _ = ActiveReviewTheme::install(ReviewTheme::default());
    let motion = WispMotion::install(reduced);
    let selected = signal(WispState::Thinking);
    let cycling = signal(false);
    let choices = STATES.map(|state| {
        view! {
            <button id={format!("state-{state:?}")} label={state.label()} size=ControlSize::Small
                    variant=ButtonVariant::Secondary keyboard_focusable
                    on_click={move || selected.set(state)}/>
        }
    });
    let controls = view! {
        <div flex flex_wrap gap=8.0>
            <button id="toggle-motion" label={move || if motion.reduced() { "Use full motion" } else { "Reduce motion" }}
                    keyboard_focusable on_click={move || motion.set_reduced(!motion.reduced())}/>
            <button id="rapid-transitions" label="Rapid transitions" keyboard_focusable
                    on_click={move || {
                        if cycling.get_untracked() { return; }
                        cycling.set(true);
                        spawn(async move {
                            for state in [WispState::Working, WispState::Writing, WispState::Thinking, WispState::Working, WispState::Writing] {
                                selected.set(state);
                                sleep(Duration::from_millis(100)).await;
                            }
                            cycling.set(false);
                        });
                    }}/>
        </div>
    };
    let sandbox = view! {
        <div flex col gap=12.0>
            <text heading>"State transitions"</text>
            <div flex items_center gap=24.0 h=64.0>
                {wisp(12.0, Color::rgb(0x0038_7ccf), move || selected.get())}
                {wisp(48.0, Color::rgb(0x0038_7ccf), move || selected.get())}
                <text>{move || selected.get().label()}</text>
            </div>
            <div flex flex_wrap gap=8.0>{..choices}</div>
            {controls}
        </div>
    };
    view! {
        <div size_full bg=0x00f6_f8fa text_color=0x001f_2328 font=UI_FONT body>
            <scroll size_full>
                <div p=24.0 flex col gap=20.0>
                    <text heading>"Activity, not progress"</text>
                    <text>"12 / 16 px compact · 48 px expanded · waiting is always still"</text>
                    <div flex gap=16.0>{palette(false)}{palette(true)}</div>
                    {sandbox}
                    <text heading>"Completion is quiet and explicit"</text>
                    <div flex gap=24.0>
                        {outcome(IconName::Check, "Completed", 0x0024_753e)}
                        {outcome(IconName::CircleX, "Failed", 0x00b4_232c)}
                        {outcome(IconName::CircleSlash, "Cancelled", 0x0059_636e)}
                    </div>
                    <text heading>"32 compact indicators · scroll to check clipping"</text>
                    <scroll h=96.0 border=1.0 border_color=0x00d1_d9e0>
                        <div flex col gap=8.0 p=8.0>{..(0..8).map(stress_row)}</div>
                    </scroll>
                    <text small>"Unfocus this window or choose reduced motion to suspend activity clocks. Hover clipping edges to check the halo."</text>
                </div>
            </scroll>
        </div>
    }
}

fn palette(dark: bool) -> impl View {
    let (bg, fg, tint, warning) = if dark {
        (0x000d_1117, 0x00e6_edf3, 0x0079_b8ff, 0x00e3_b341)
    } else {
        (0x00ff_ffff, 0x001f_2328, 0x0028_64b4, 0x0094_6200)
    };
    let rows = STATES.map(|state| {
        let tint = Color::rgb(if state == WispState::Waiting { warning } else { tint });
        view! {
            <div flex items_center gap=24.0 h=48.0>
                <div w=144.0><text>{state.label()}</text></div>
                {wisp(12.0, tint, move || state)}
                {wisp(16.0, tint, move || state)}
                {wisp(48.0, tint, move || state)}
            </div>
        }
    });
    view! {
        <div flex_1 min_w=0.0 p=16.0 bg=bg text_color=fg flex col gap=8.0>
            <text label>{if dark { "Dark" } else { "Light" }}</text>
            {..rows}
        </div>
    }
}

fn outcome(icon: IconName, label: &'static str, color: u32) -> impl View {
    view! { <div flex items_center gap=8.0>{Icon::new(icon, Color::rgb(color)).size(12.0)}<text>{label}</text></div> }
}

fn stress_row(row: usize) -> impl View {
    view! {
        <div flex items_center gap=24.0 h=20.0>
            {..(0..4).map(move |index| wisp(12.0, Color::rgb(0x0038_7ccf), move || STATES[(row + index) % 5]))}
            <text>{format!("Tool group {}", row + 1)}</text>
        </div>
    }
}

const STATES: [WispState; 6] = [
    WispState::Connecting,
    WispState::Thinking,
    WispState::Compacting,
    WispState::Working,
    WispState::Writing,
    WispState::Waiting,
];
