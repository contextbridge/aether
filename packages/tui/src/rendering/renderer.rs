use super::frame::Frame;
use super::line::Line;
use super::render_context::ViewContext;
use super::terminal_screen::{TerminalCommand, TerminalScreen};
use super::visual_frame::VisualFrame;
use crate::rendering::render_context::Size;
use crate::theme::Theme;
use std::io::{self, Write};
use std::sync::Arc;

#[cfg(feature = "syntax")]
use crate::syntax_highlighting::SyntaxHighlighter;

pub enum RendererCommand {
    Bell,
    ClearScreen,
    SetTheme(Theme),
    SetMouseCapture(bool),
}

#[doc = include_str!("../docs/renderer.md")]
pub struct Renderer<W: Write> {
    terminal: TerminalScreen<W>,
    size: Size,
    theme: Arc<Theme>,
    #[cfg(feature = "syntax")]
    highlighter: Arc<SyntaxHighlighter>,
    prev_frame: Option<VisualFrame>,
    resized: bool,
}

impl<W: Write> Renderer<W> {
    pub fn new(writer: W, theme: Theme, size: impl Into<Size>) -> Self {
        Self {
            terminal: TerminalScreen::new(writer),
            size: size.into(),
            theme: Arc::new(theme),
            #[cfg(feature = "syntax")]
            highlighter: Arc::new(SyntaxHighlighter::new()),
            prev_frame: None,
            resized: false,
        }
    }

    /// Render a frame using a closure.
    ///
    /// The closure receives a `ViewContext` and returns a Frame.
    pub fn render_frame(&mut self, f: impl FnOnce(&ViewContext) -> Frame) -> io::Result<()> {
        let context = self.context();
        let frame = f(&context).clamp_cursor();
        self.render_frame_internal(frame)
    }

    pub fn cleanup(&mut self) -> io::Result<()> {
        let visible_rows = self.prev_frame.as_ref().map_or(0, |f| f.visible_lines().len());
        self.terminal.cleanup(visible_rows)?;
        self.prev_frame = None;
        Ok(())
    }

    pub fn clear_screen(&mut self) -> io::Result<()> {
        let commands = vec![TerminalCommand::ClearAll];
        self.terminal.execute_batch(&commands)?;
        self.prev_frame = None;
        self.resized = false;
        Ok(())
    }

    pub fn push_to_scrollback(&mut self, lines: &[Line]) -> io::Result<()> {
        self.push_lines_to_scrollback(lines, self.size.width)
    }

    pub fn on_resize(&mut self, size: impl Into<Size>) {
        self.size = size.into();
        self.terminal.reset_cursor_offset();
        self.prev_frame = None;
        self.resized = true;
    }

    pub fn context(&self) -> ViewContext {
        ViewContext {
            size: self.size,
            theme: self.theme.clone(),
            #[cfg(feature = "syntax")]
            highlighter: self.highlighter.clone(),
        }
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = Arc::new(theme);
    }

    pub fn apply_commands(&mut self, commands: Vec<RendererCommand>) -> io::Result<()> {
        for cmd in commands {
            match cmd {
                RendererCommand::Bell => self.terminal.execute(&TerminalCommand::Bell)?,
                RendererCommand::ClearScreen => self.clear_screen()?,
                RendererCommand::SetTheme(theme) => self.set_theme(theme),
                RendererCommand::SetMouseCapture(enable) => {
                    self.terminal.execute(&TerminalCommand::SetMouseCapture(enable))?;
                    self.terminal.writer.flush()?;
                }
            }
        }
        Ok(())
    }

    pub fn writer(&self) -> &W {
        &self.terminal.writer
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn test_writer_mut(&mut self) -> &mut W {
        &mut self.terminal.writer
    }

    #[cfg(test)]
    fn committed_scrollback_count(&self) -> usize {
        self.prev_frame.as_ref().map_or(0, |f| f.scrollback_lines().len())
    }

    fn render_frame_internal(&mut self, frame: Frame) -> io::Result<()> {
        let next_frame = VisualFrame::from_frame(frame, self.size);
        let next_scrollback = next_frame.scrollback_lines();
        let prev = self.prev_frame.as_ref();
        let prev_scrollback = prev.map_or(&[][..], VisualFrame::scrollback_lines);
        let can_incremental = !self.resized && next_scrollback.starts_with(prev_scrollback);
        let (mut commands, mut prev_for_diff, new_scrollback) = if can_incremental {
            (vec![TerminalCommand::RestoreCursorPosition], prev, &next_scrollback[prev_scrollback.len()..])
        } else {
            (vec![TerminalCommand::ClearAll], None, next_scrollback)
        };

        if !new_scrollback.is_empty() {
            commands.push(TerminalCommand::PushScrollbackLines {
                previous_visible_rows: prev_for_diff.map_or(0, |f| f.visible_lines().len()),
                lines: new_scrollback,
            });
            prev_for_diff = None;
        }

        if let Some(diff) = VisualFrame::diff(prev_for_diff, &next_frame) {
            commands.push(Self::rewrite_command(&diff));
        }

        commands.extend(Self::cursor_commands(&next_frame));
        self.terminal.execute_batch(&commands)?;
        self.prev_frame = Some(next_frame);
        self.resized = false;
        Ok(())
    }

    fn rewrite_command<'a>(diff: &super::visual_frame::LineDiff<'a>) -> TerminalCommand<'a> {
        TerminalCommand::RewriteVisibleLines {
            rows_up: diff_rows_up(diff),
            append_after_existing: diff_should_append(diff),
            lines: diff.lines,
        }
    }

    fn cursor_commands(frame: &VisualFrame) -> [TerminalCommand<'_>; 2] {
        let cursor = frame.cursor();
        let rows_up = to_u16(frame.visible_lines().len().saturating_sub(1).saturating_sub(cursor.row));

        [
            TerminalCommand::SetCursorVisible(cursor.is_visible),
            TerminalCommand::PlaceCursor { rows_up, col: to_u16(cursor.col) },
        ]
    }

    fn push_lines_to_scrollback(&mut self, lines: &[Line], width: u16) -> io::Result<()> {
        use super::visual_frame::prepare_lines_for_scrollback;

        let visual = prepare_lines_for_scrollback(lines, width);
        let previous_visible_rows = self.prev_frame.as_ref().map_or(0, |f| f.visible_lines().len());
        self.prev_frame = None;

        if visual.is_empty() {
            return Ok(());
        }

        self.terminal.execute_batch(&[
            TerminalCommand::RestoreCursorPosition,
            TerminalCommand::PushScrollbackLines { previous_visible_rows, lines: &visual },
        ])
    }
}

fn diff_rows_up(diff: &super::visual_frame::LineDiff<'_>) -> u16 {
    if diff.rewrite_from < diff.previous_row_count {
        to_u16(diff.previous_row_count - 1 - diff.rewrite_from)
    } else {
        0
    }
}

fn diff_should_append(diff: &super::visual_frame::LineDiff<'_>) -> bool {
    diff.rewrite_from >= diff.previous_row_count && diff.previous_row_count > 0
}

fn to_u16(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rendering::frame::Cursor;

    struct FakeWriter {
        bytes: Vec<u8>,
    }

    impl FakeWriter {
        fn new() -> Self {
            Self { bytes: Vec::new() }
        }
    }

    impl Write for FakeWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn renderer(size: (u16, u16)) -> Renderer<FakeWriter> {
        Renderer::new(FakeWriter::new(), Theme::default(), size)
    }

    fn output(r: &Renderer<FakeWriter>) -> String {
        String::from_utf8_lossy(&r.terminal.writer.bytes).into_owned()
    }

    fn frame(lines: &[&str]) -> Frame {
        Frame::new(lines.iter().map(|line| Line::new(*line)).collect()).with_cursor(Cursor {
            row: lines.len().saturating_sub(1),
            col: 0,
            is_visible: true,
        })
    }

    fn frame_with_cursor(lines: &[&str], row: usize, col: usize) -> Frame {
        Frame::new(lines.iter().map(|line| Line::new(*line)).collect()).with_cursor(Cursor {
            row,
            col,
            is_visible: true,
        })
    }

    /// Render `first`, clear output buffer, render `second`, return the output.
    fn diff_output(r: &mut Renderer<FakeWriter>, first: Frame, second: Frame) -> String {
        r.render_frame_internal(first).unwrap();
        r.terminal.writer.bytes.clear();
        r.render_frame_internal(second).unwrap();
        output(r)
    }

    fn assert_has(output: &str, needle: &str, msg: &str) {
        assert!(output.contains(needle), "{msg}: {output:?}");
    }

    fn assert_missing(output: &str, needle: &str, msg: &str) {
        assert!(!output.contains(needle), "{msg}: {output:?}");
    }

    #[test]
    fn set_theme_replaces_render_context_theme() {
        let mut r = Renderer::new(Vec::new(), Theme::default(), (80, 24));
        let new_theme = Theme::default();
        let expected = new_theme.text_primary();
        r.set_theme(new_theme);
        assert_eq!(r.context().theme.text_primary(), expected);
    }

    #[cfg(feature = "syntax")]
    #[test]
    fn set_theme_replaces_render_context_theme_from_file() {
        let mut r = Renderer::new(Vec::new(), Theme::default(), (80, 24));
        let custom_tmtheme = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>name</key>
    <string>Custom</string>
    <key>settings</key>
    <array>
        <dict>
            <key>settings</key>
            <dict>
                <key>foreground</key>
                <string>#112233</string>
                <key>background</key>
                <string>#000000</string>
            </dict>
        </dict>
    </array>
</dict>
</plist>"#;
        let temp_dir = tempfile::TempDir::new().unwrap();
        let theme_path = temp_dir.path().join("custom.tmTheme");
        std::fs::write(&theme_path, custom_tmtheme).unwrap();
        r.set_theme(Theme::load_from_path(&theme_path));
        assert_eq!(r.context().theme.text_primary(), crossterm::style::Color::Rgb { r: 0x11, g: 0x22, b: 0x33 });
    }

    #[test]
    fn identical_rerender_produces_no_content_output() {
        for lines in [vec![], vec!["hello", "world"]] {
            let mut r = renderer((80, 24));
            let f = frame(&lines);
            let out = diff_output(&mut r, f.clone(), f);
            for word in &lines {
                assert_missing(&out, word, "identical re-render should not rewrite content");
            }
            assert_missing(&out, "\x1b[J", "should not clear from cursor down");
        }
    }

    #[test]
    fn first_render_writes_all_lines() {
        let mut r = renderer((80, 24));
        let f = frame(&["hello", "world"]);
        r.render_frame_internal(f).unwrap();
        let out = output(&r);
        for word in ["hello", "world"] {
            assert_has(&out, word, "first render should contain line");
        }
    }

    #[test]
    fn changing_middle_line_rewrites_from_diff() {
        let mut r = renderer((80, 24));
        let out = diff_output(&mut r, frame(&["aaa", "bbb", "ccc"]), frame(&["aaa", "BBB", "ccc"]));
        for word in ["BBB", "ccc"] {
            assert_has(&out, word, "changed/subsequent lines should be rewritten");
        }
    }

    #[test]
    fn appending_line_moves_to_next_row_before_writing() {
        let mut r = renderer((80, 24));
        let out = diff_output(&mut r, frame(&["aaa", "bbb"]), frame(&["aaa", "bbb", "ccc"]));
        let ccc_pos = out.find("ccc").expect("missing appended line");
        assert!(out[..ccc_pos].contains("\r\n"), "should move to next row before appending: {out:?}");
    }

    #[test]
    fn push_to_scrollback_restores_cursor_even_when_nothing_new_is_flushed() {
        let mut r = renderer((80, 2));
        let f = frame_with_cursor(&["L1", "L2", "L3", "L4"], 2, 0);
        r.render_frame_internal(f).unwrap();
        r.terminal.writer.bytes.clear();
        r.push_to_scrollback(&[Line::new("already flushed 1"), Line::new("already flushed 2")]).unwrap();
        assert_has(&output(&r), "\x1b[1B", "should restore cursor before early return");
    }

    #[test]
    fn push_to_scrollback_clears_prev_visible_lines() {
        let mut r = renderer((80, 24));
        let f = frame(&["managed line"]);
        r.render_frame_internal(f.clone()).unwrap();
        r.push_to_scrollback(&[Line::new("scrolled")]).unwrap();
        r.terminal.writer.bytes.clear();
        r.render_frame_internal(f).unwrap();
        assert_has(&output(&r), "managed line", "should re-render managed content after scrollback");
    }

    #[test]
    fn push_to_scrollback_empty_is_noop() {
        let mut r = renderer((80, 24));
        r.push_to_scrollback(&[]).unwrap();
        assert!(r.terminal.writer.bytes.is_empty());
    }

    #[test]
    fn clear_screen_emits_clear_all_and_purge() {
        let mut r = renderer((80, 24));
        r.clear_screen().unwrap();
        let out = output(&r);
        for (seq, label) in [("\x1b[2J", "ClearAll"), ("\x1b[3J", "Purge"), ("\x1b[1;1H", "cursor home")] {
            assert_has(&out, seq, &format!("missing {label}"));
        }
    }

    #[test]
    fn clear_screen_resets_resize_state() {
        let mut r = renderer((80, 24));
        r.clear_screen().unwrap();
        r.terminal.writer.bytes.clear();
        r.render_frame_internal(frame(&["hello"])).unwrap();
        let out = output(&r);
        assert_has(&out, "hello", "should render content");
        assert_missing(&out, "\x1b[2J", "render after clear_screen should not re-clear viewport");
    }

    #[test]
    fn resize_marks_terminal_for_full_clear_and_redraw() {
        let mut r = renderer((10, 4));
        r.render_frame_internal(frame(&["abcdefghij"])).unwrap();
        r.terminal.writer.bytes.clear();
        r.on_resize((5, 4));
        r.render_frame_internal(frame(&["abcdefghij"])).unwrap();
        let out = output(&r);
        for (seq, label) in
            [("\x1b[2J", "ClearAll"), ("\x1b[3J", "Purge"), ("abcde", "wrapped-1"), ("fghij", "wrapped-2")]
        {
            assert_has(&out, seq, &format!("resize should emit {label}"));
        }
    }

    #[test]
    fn on_resize_resets_prev_frame() {
        let mut r = renderer((80, 24));
        r.render_frame_internal(frame(&["hello"])).unwrap();
        assert!(r.prev_frame.is_some());
        r.on_resize((40, 12));
        assert!(r.prev_frame.is_none(), "on_resize should reset prev_frame");
    }

    #[test]
    fn visual_frame_splits_overflow_from_visible_lines() {
        let mut r = renderer((80, 2));
        let f = frame_with_cursor(&["L1", "L2", "L3", "L4"], 3, 0);
        r.render_frame_internal(f).unwrap();
        assert_eq!(r.committed_scrollback_count(), 2);
    }

    #[test]
    fn cursor_remapped_after_wrap() {
        let mut r = renderer((3, 24));
        let f = frame_with_cursor(&["abcdef"], 0, 5);
        r.render_frame_internal(f).unwrap();
        assert_has(&output(&r), "\x1b[2C", "cursor should be at col 2 (MoveRight(2))");
    }

    fn cleanup_output(r: &mut Renderer<FakeWriter>) -> String {
        r.terminal.writer.bytes.clear();
        r.cleanup().unwrap();
        output(r)
    }

    #[test]
    fn cleanup_clears_rendered_region() {
        let mut r = renderer((80, 24));
        r.render_frame_internal(frame(&["aaa", "bbb", "ccc"])).unwrap();
        let out = cleanup_output(&mut r);
        assert_has(&out, "\x1b[2A", "should move up to top of rendered region");
        assert_has(&out, "\x1b[J", "should clear from cursor down");
    }

    #[test]
    fn cleanup_restores_cursor_offset() {
        let mut r = renderer((80, 24));
        let f = frame_with_cursor(&["aaa", "bbb", "ccc"], 0, 0);
        r.render_frame_internal(f).unwrap();
        let out = cleanup_output(&mut r);
        assert_has(&out, "\x1b[2B", "should move down past cursor offset");
        assert_has(&out, "\x1b[2A", "should then move up to top");
        assert_has(&out, "\x1b[J", "should clear from cursor down");
    }

    #[test]
    fn cleanup_shows_hidden_cursor() {
        let mut r = renderer((80, 24));
        let f = Frame::new(vec![Line::new("hello")]).with_cursor(Cursor::hidden());
        r.render_frame_internal(f).unwrap();
        let out = cleanup_output(&mut r);
        assert_has(&out, "\x1b[?25h", "should show cursor");
    }

    #[test]
    fn cleanup_without_render_does_not_move_cursor() {
        let mut r = renderer((80, 24));
        let out = cleanup_output(&mut r);
        for n in 1..=10 {
            assert_missing(&out, &format!("\x1b[{n}A"), "should not move up");
            assert_missing(&out, &format!("\x1b[{n}B"), "should not move down");
        }
        assert_missing(&out, "\x1b[?25h", "should not show cursor (already visible)");
    }

    #[test]
    fn cleanup_after_clear_screen_is_noop() {
        let mut r = renderer((80, 24));
        r.render_frame_internal(frame(&["aaa", "bbb"])).unwrap();
        r.clear_screen().unwrap();
        let out = cleanup_output(&mut r);
        assert_missing(&out, "\x1b[2A", "should not move up");
        assert_missing(&out, "\x1b[2B", "should not move down");
    }

    #[test]
    fn cleanup_resets_prev_frame() {
        let mut r = renderer((80, 24));
        r.render_frame_internal(frame(&["hello"])).unwrap();
        r.cleanup().unwrap();
        assert!(r.prev_frame.is_none(), "cleanup should reset prev_frame");
    }
}
