use crossterm::QueueableCommand;
use crossterm::cursor::{Hide, MoveDown, MoveRight, MoveTo, MoveUp, Show};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::terminal::{BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate};
use std::io::{self, Write};

use super::line::Line;
use crate::terminal_codes::BELL;

pub(crate) enum TerminalCommand<'a> {
    Bell,
    ClearAll,
    SetCursorVisible(bool),
    SetMouseCapture(bool),
    RestoreCursorPosition,
    PlaceCursor { rows_up: u16, col: u16 },
    RewriteVisibleLines { rows_up: u16, append_after_existing: bool, lines: &'a [Line] },
    PushScrollbackLines { previous_visible_rows: usize, lines: &'a [Line] },
}

pub(crate) struct TerminalScreen<W: Write> {
    pub(super) writer: W,
    cursor_row_offset: u16,
    cursor_visible: bool,
    mouse_captured: bool,
}

impl<W: Write> TerminalScreen<W> {
    pub(crate) fn new(writer: W) -> Self {
        Self { writer, cursor_row_offset: 0, cursor_visible: true, mouse_captured: false }
    }

    pub(crate) fn execute_batch(&mut self, commands: &[TerminalCommand<'_>]) -> io::Result<()> {
        self.writer.queue(BeginSynchronizedUpdate)?;
        for command in commands {
            self.execute(command)?;
        }
        self.writer.queue(EndSynchronizedUpdate)?;
        self.writer.flush()
    }

    pub(crate) fn execute(&mut self, command: &TerminalCommand<'_>) -> io::Result<()> {
        match command {
            TerminalCommand::Bell => {
                self.writer.write_all(&[BELL])?;
                self.writer.flush()?;
            }
            TerminalCommand::ClearAll => {
                self.writer.queue(Clear(ClearType::All))?;
                self.writer.queue(Clear(ClearType::Purge))?;
                self.writer.queue(MoveTo(0, 0))?;
                self.cursor_row_offset = 0;
            }
            TerminalCommand::SetCursorVisible(visible) => {
                if *visible != self.cursor_visible {
                    if *visible {
                        self.writer.queue(Show)?;
                    } else {
                        self.writer.queue(Hide)?;
                    }
                    self.cursor_visible = *visible;
                }
            }
            TerminalCommand::SetMouseCapture(enable) => {
                if *enable != self.mouse_captured {
                    if *enable {
                        self.writer.queue(EnableMouseCapture)?;
                    } else {
                        self.writer.queue(DisableMouseCapture)?;
                    }
                    self.mouse_captured = *enable;
                }
            }
            TerminalCommand::PlaceCursor { rows_up, col } => {
                self.writer.queue(MoveUp(*rows_up))?;
                write!(self.writer, "\r")?;
                if *col > 0 {
                    self.writer.queue(MoveRight(*col))?;
                }
                self.cursor_row_offset = *rows_up;
            }
            TerminalCommand::RestoreCursorPosition => {
                if self.cursor_row_offset > 0 {
                    self.writer.queue(MoveDown(self.cursor_row_offset))?;
                    self.cursor_row_offset = 0;
                }
            }
            TerminalCommand::RewriteVisibleLines { rows_up, append_after_existing, lines } => {
                if *rows_up > 0 {
                    self.writer.queue(MoveUp(*rows_up))?;
                    write!(self.writer, "\r")?;
                } else if *append_after_existing {
                    write!(self.writer, "\r\n")?;
                } else {
                    write!(self.writer, "\r")?;
                }
                self.writer.queue(Clear(ClearType::FromCursorDown))?;
                for (i, line) in lines.iter().enumerate() {
                    write!(self.writer, "{}", line.to_ansi_string())?;
                    if i < lines.len() - 1 {
                        write!(self.writer, "\r\n")?;
                    }
                }
            }
            TerminalCommand::PushScrollbackLines { previous_visible_rows, lines } => {
                if *previous_visible_rows > 1 {
                    let rows_up = u16::try_from(previous_visible_rows - 1).unwrap_or(u16::MAX);
                    self.writer.queue(MoveUp(rows_up))?;
                }
                write!(self.writer, "\r")?;
                self.writer.queue(Clear(ClearType::FromCursorDown))?;
                for line in *lines {
                    write!(self.writer, "{}", line.to_ansi_string())?;
                    write!(self.writer, "\r\n")?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn cleanup(&mut self, visible_rows: usize) -> io::Result<()> {
        if self.cursor_row_offset > 0 {
            self.writer.queue(MoveDown(self.cursor_row_offset))?;
        }
        let up = visible_rows.saturating_sub(1);
        if up > 0 {
            self.writer.queue(MoveUp(u16::try_from(up).unwrap_or(u16::MAX)))?;
        }
        write!(self.writer, "\r")?;
        self.writer.queue(Clear(ClearType::FromCursorDown))?;
        if !self.cursor_visible {
            self.writer.queue(Show)?;
            self.cursor_visible = true;
        }
        self.cursor_row_offset = 0;
        self.writer.flush()
    }

    pub(crate) fn reset_cursor_offset(&mut self) {
        self.cursor_row_offset = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn run_batch(commands: &[TerminalCommand<'_>]) -> (TerminalScreen<FakeWriter>, String) {
        let mut screen = TerminalScreen::new(FakeWriter::new());
        screen.execute_batch(commands).unwrap();
        let content = extract_sync_content(&screen);
        (screen, content)
    }

    fn run_one(command: TerminalCommand<'_>) -> String {
        run_batch(&[command]).1
    }

    fn continue_batch(screen: &mut TerminalScreen<FakeWriter>, commands: &[TerminalCommand<'_>]) -> String {
        screen.writer.bytes.clear();
        screen.execute_batch(commands).unwrap();
        extract_sync_content(screen)
    }

    fn extract_sync_content(screen: &TerminalScreen<FakeWriter>) -> String {
        let output = String::from_utf8_lossy(&screen.writer.bytes).into_owned();
        let begin_tag = "\x1b[?2026h";
        let end_tag = "\x1b[?2026l";
        if let (Some(begin), Some(end)) = (output.find(begin_tag), output.find(end_tag)) {
            let start = begin + begin_tag.len();
            if start <= end {
                return output[start..end].to_string();
            }
        }
        output
    }

    fn assert_no_move_up(content: &str) {
        for n in 1..=10 {
            assert!(!content.contains(&format!("\x1b[{n}A")), "unexpected MoveUp({n}) in: {content:?}");
        }
    }

    fn assert_has_all(content: &str, substrings: &[&str]) {
        for s in substrings {
            assert!(content.contains(s), "missing {s:?} in: {content:?}");
        }
    }

    #[test]
    fn clear_all_emits_expected_sequences() {
        let content = run_one(TerminalCommand::ClearAll);
        assert_has_all(&content, &["\x1b[2J", "\x1b[3J", "\x1b[1;1H"]);
    }

    #[test]
    fn set_cursor_visible_only_writes_on_state_change() {
        let (mut screen, content) = run_batch(&[TerminalCommand::SetCursorVisible(true)]);
        assert!(content.is_empty(), "no-op: already visible");

        let content = continue_batch(&mut screen, &[TerminalCommand::SetCursorVisible(false)]);
        assert!(content.contains("\x1b[?25l"), "missing Hide");

        let content = continue_batch(&mut screen, &[TerminalCommand::SetCursorVisible(false)]);
        assert!(content.is_empty(), "no-op: already hidden");

        let content = continue_batch(&mut screen, &[TerminalCommand::SetCursorVisible(true)]);
        assert!(content.contains("\x1b[?25h"), "missing Show");
    }

    #[test]
    fn place_cursor_and_restore_round_trip() {
        let (mut screen, content) = run_batch(&[TerminalCommand::PlaceCursor { rows_up: 3, col: 5 }]);
        assert_has_all(&content, &["\x1b[3A", "\r", "\x1b[5C"]);

        let content = continue_batch(&mut screen, &[TerminalCommand::RestoreCursorPosition]);
        assert!(content.contains("\x1b[3B"), "missing MoveDown(3)");

        let content = continue_batch(&mut screen, &[TerminalCommand::RestoreCursorPosition]);
        assert!(content.is_empty(), "no-op: offset already 0");
    }

    #[test]
    fn execute_wraps_commands_in_synchronized_update() {
        let mut screen = TerminalScreen::new(FakeWriter::new());
        screen.execute_batch(&[TerminalCommand::ClearAll]).unwrap();
        let output = String::from_utf8_lossy(&screen.writer.bytes).into_owned();
        let begin = output.find("\x1b[?2026h").expect("missing Begin");
        let clear = output.find("\x1b[2J").expect("missing Clear");
        let end = output.find("\x1b[?2026l").expect("missing End");
        assert!(begin < clear && clear < end, "wrong ordering");
    }

    #[test]
    fn rewrite_visible_lines_emits_expected_sequences() {
        let lines = vec![Line::new("line1"), Line::new("line2"), Line::new("line3")];
        let content =
            run_one(TerminalCommand::RewriteVisibleLines { rows_up: 2, append_after_existing: false, lines: &lines });

        assert_has_all(&content, &["\x1b[2A", "\r", "\x1b[J", "line1", "line2", "line3"]);

        // Newlines between lines but not after last
        let p1 = content.find("line1").unwrap();
        let p2 = content.find("line2").unwrap();
        let p3 = content.find("line3").unwrap();
        assert!(content[p1 + 5..p2].contains("\r\n"), "missing newline between 1-2");
        assert!(content[p2 + 5..p3].contains("\r\n"), "missing newline between 2-3");
        assert!(!content[p3 + 5..].starts_with("\r\n"), "unexpected trailing newline");
    }

    #[test]
    fn rewrite_visible_lines_with_zero_rows_up() {
        let lines = vec![Line::new("only")];
        let content =
            run_one(TerminalCommand::RewriteVisibleLines { rows_up: 0, append_after_existing: false, lines: &lines });
        assert_no_move_up(&content);
        assert!(content.contains("only"), "missing content");
    }

    #[test]
    fn rewrite_visible_lines_append_moves_to_next_row_before_clearing() {
        let lines = vec![Line::new("appended")];
        let content =
            run_one(TerminalCommand::RewriteVisibleLines { rows_up: 0, append_after_existing: true, lines: &lines });
        assert!(content.starts_with("\r\n\x1b[Jappended"), "expected newline before clear, got: {content:?}");
    }

    #[test]
    fn push_scrollback_lines_emits_expected_sequences() {
        let lines = vec![Line::new("scroll1"), Line::new("scroll2")];
        let content = run_one(TerminalCommand::PushScrollbackLines { previous_visible_rows: 4, lines: &lines });
        assert_has_all(&content, &["\x1b[3A", "\r", "\x1b[J", "scroll1", "scroll2"]);
        assert!(content.ends_with("\r\n"), "should end with newline");
    }

    #[test]
    fn push_scrollback_lines_skips_move_up_for_small_previous_rows() {
        for prev_rows in [0, 1] {
            let lines = vec![Line::new("scroll")];
            let content =
                run_one(TerminalCommand::PushScrollbackLines { previous_visible_rows: prev_rows, lines: &lines });
            assert_no_move_up(&content);
            assert_has_all(&content, &["scroll", "\r\n"]);
        }
    }
}
