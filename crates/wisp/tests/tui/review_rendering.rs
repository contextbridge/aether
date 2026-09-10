use std::{path::PathBuf, sync::Arc};

use clankerdiff_core::DiffScope;
use clankerdiff_git::RepositorySnapshot;
use ratatui::{
    buffer::{Buffer, Cell},
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
};
use utils::plan_review::PlanReviewElicitationMeta;
use wisp::{
    command::GitCommand,
    git_review::{DiffDocument, FileDiff, GitDiffEvent},
    renderer::DrawContext,
    screens::{git_diff::GitDiffScreen, plan_review::PlanReviewScreen},
    surfaces::elicitation::ElicitationResponder,
    theme::Theme,
    view::{generation::Generation, syntax::SyntaxHighlighter},
};

#[test]
fn git_review_clears_underlying_content_only_inside_its_viewport() {
    for y in [0, 2] {
        for lines in [1, 60] {
            let new = "fn added() {}\n".repeat(lines);
            let (mut screen, command) = GitDiffScreen::new(PathBuf::from("/workspace"));
            let GitCommand::Load { request_id, .. } = command else {
                panic!("opening review must load the repository");
            };
            screen.on_event(GitDiffEvent::Loaded {
                request_id,
                result: Ok(RepositorySnapshot {
                    scope: DiffScope::Both,
                    document: Arc::new(DiffDocument {
                        repo_root: "/workspace".into(),
                        files: vec![FileDiff::from_texts("src/lib.rs", "", &new).unwrap()],
                    }),
                }),
            });
            assert_opaque_viewport(Rect::new(3, y, 120, 24), |area, buffer, cx| {
                screen.render(area, buffer, cx);
            });
        }
    }
}

#[test]
fn plan_review_clears_underlying_content_only_inside_its_viewport() {
    for y in [0, 2] {
        for lines in [1, 60] {
            let markdown = format!("# Plan\n\n{}", "Implement this.\n\n".repeat(lines));
            let mut screen = PlanReviewScreen::new(
                PlanReviewElicitationMeta::new(&PathBuf::from("/workspace/plan.md"), &markdown),
                ElicitationResponder::from_fn(|_| {}),
            );
            assert_opaque_viewport(Rect::new(3, y, 120, 24), |area, buffer, cx| {
                screen.render(area, buffer, cx);
            });
        }
    }
}

fn assert_opaque_viewport(area: Rect, mut render: impl FnMut(Rect, &mut Buffer, &mut DrawContext<'_>)) {
    let canvas = Rect::new(0, 0, 126, 28);
    let theme = Theme::default();
    let mut highlighter = SyntaxHighlighter::new();
    let mut cx = DrawContext { theme: &theme, highlighter: &mut highlighter, theme_generation: Generation::default() };
    let mut clean = Buffer::empty(canvas);
    render(area, &mut clean, &mut cx);
    let mut marker = Cell::default();
    marker.set_symbol("▓").set_style(Style::new().fg(Color::White).bg(Color::Red).add_modifier(Modifier::UNDERLINED));
    let mut dirty = Buffer::filled(canvas, marker.clone());
    render(area, &mut dirty, &mut cx);

    for y in canvas.top()..canvas.bottom() {
        for x in canvas.left()..canvas.right() {
            let position = Position::new(x, y);
            if area.contains(position) {
                assert_eq!(
                    dirty[position].symbol(),
                    clean[position].symbol(),
                    "underlying text leaked at {position:?}"
                );
            } else {
                assert_eq!(dirty[position], marker, "review changed a cell outside {area:?} at {position:?}");
            }
        }
    }
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let position = Position::new(x, y);
            assert_eq!(dirty[position], clean[position], "underlying content leaked into {area:?} at {position:?}");
        }
    }
    for x in area.left() + 1..area.right() - 1 {
        assert_eq!(dirty[(x, area.bottom() - 1)].symbol(), "─", "review content must not overwrite its bottom border");
    }
}
