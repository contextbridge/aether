use super::support::{
    Buffer, Cell, Color, Modifier, Position, block_on_local, git_diff_screen, plan_meta, render_surface,
    test_connection,
};
use ratatui::layout::Rect;
use wisp::git_review::FileDiff;
use wisp::renderer::DrawContext;
use wisp::screens::artifact_review::ArtifactReviewScreen;

#[test]
fn git_review_clears_underlying_content_only_inside_its_viewport() {
    for y in [0, 2] {
        for lines in [1, 60] {
            let new = "fn added() {}\n".repeat(lines);
            let mut screen = git_diff_screen(vec![FileDiff::from_texts("src/lib.rs", "", &new).unwrap()]);
            assert_opaque_viewport(Rect::new(3, y, 120, 24), |area, buffer, cx| {
                screen.render(area, buffer, cx);
            });
        }
    }
}

#[test]
fn artifact_review_clears_underlying_content_only_inside_its_viewport() {
    block_on_local(async {
        let (cx, mut peer) = test_connection().await;
        for y in [0, 2] {
            for lines in [1, 60] {
                let markdown = format!("# Plan\n\n{}", "Implement this.\n\n".repeat(lines));
                let (responder, _response) = peer.fake_elicitation(&cx).await;
                let mut screen = ArtifactReviewScreen::new(plan_meta(&markdown), responder.into());
                assert_opaque_viewport(Rect::new(3, y, 120, 24), |area, buffer, cx| {
                    screen.render(area, buffer, cx);
                });
                screen.on_ui_event(&wisp::view::input::UiEvent::Key(crossterm::event::KeyCode::Char('r').into()));
                assert_opaque_viewport(Rect::new(3, y, 120, 24), |area, buffer, cx| {
                    assert_eq!(screen.render(area, buffer, cx), None);
                });
            }
        }
    });
}

fn assert_opaque_viewport(area: Rect, mut render: impl FnMut(Rect, &mut Buffer, &mut DrawContext<'_>)) {
    let canvas = Rect::new(0, 0, 126, 28);
    let clean = render_surface(Buffer::empty(canvas), |buffer, cx| render(area, buffer, cx));
    let mut marker = Cell::default();
    marker.set_symbol("▓").set_style((Color::White, Color::Red, Modifier::UNDERLINED));
    let dirty = render_surface(Buffer::filled(canvas, marker.clone()), |buffer, cx| render(area, buffer, cx));
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
