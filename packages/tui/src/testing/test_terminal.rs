use crossterm::style::{Color, force_color_output};
use std::io::{self, Write};

use crate::Style;
use crate::terminal_codes::BELL;

/// A single cell in the terminal buffer, storing both a character and its style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Self { ch: ' ', style: Style::default() }
    }
}

impl Cell {
    fn new(ch: char, style: Style) -> Self {
        Self { ch, style }
    }
}

/// A virtual terminal buffer for testing terminal output.
/// Captures all writes, tracks cursor position, and parses ANSI escape sequences
/// including SGR (Select Graphic Rendition) codes for style tracking.
///
/// Implements delayed wrapping (DEC-style): when the cursor reaches the last
/// column, it stays there with a pending-wrap flag. The next printable character
/// triggers the wrap to column 0 of the next line. A `\r` clears the flag.
#[derive(Debug, Clone)]
pub struct TestTerminal {
    /// 2D buffer of cells (row, column)
    buffer: Vec<Vec<Cell>>,
    /// Rows that have scrolled off the top of the visible buffer.
    scrollback: Vec<Vec<Cell>>,
    /// Current cursor position (column, row)
    cursor: (u16, u16),
    /// Saved cursor position (for save/restore)
    saved_cursor: Option<(u16, u16)>,
    /// Terminal size (columns, rows)
    size: (u16, u16),
    /// Buffer for incomplete escape sequences
    escape_buffer: Vec<u8>,
    /// Delayed wrap: cursor hit last column but hasn't wrapped yet
    pending_wrap: bool,
    /// Current SGR style applied to newly written characters
    current_style: Style,
    bell_count: usize,
}

impl TestTerminal {
    /// Create a new test terminal with given size
    pub fn new(columns: u16, rows: u16) -> Self {
        force_color_output(true);
        let buffer = vec![vec![Cell::default(); columns as usize]; rows as usize];
        Self {
            buffer,
            scrollback: Vec::new(),
            cursor: (0, 0),
            saved_cursor: None,
            size: (columns, rows),
            escape_buffer: Vec::new(),
            pending_wrap: false,
            current_style: Style::default(),
            bell_count: 0,
        }
    }

    pub fn bell_count(&self) -> usize {
        self.bell_count
    }

    /// Resize terminal without preserving prior transcript content.
    pub fn resize(&mut self, columns: u16, rows: u16) {
        let columns = columns.max(1);
        let rows = rows.max(1);
        self.buffer = vec![vec![Cell::default(); columns as usize]; rows as usize];
        self.scrollback.clear();
        self.size = (columns, rows);
        self.cursor = (0, rows.saturating_sub(1));
        self.saved_cursor = None;
        self.pending_wrap = false;
    }

    /// Resize terminal and reflow existing transcript content to match the new width.
    pub fn resize_preserving_transcript(&mut self, columns: u16, rows: u16) {
        let transcript = self.get_transcript_lines();
        let wrapped = Self::reflow_lines(&transcript, columns);
        self.apply_reflowed_lines(columns, rows, &wrapped);
    }

    fn reflow_lines(lines: &[String], columns: u16) -> Vec<String> {
        let mut wrapped = Vec::new();
        let width = columns.max(1) as usize;

        for line in lines {
            if line.is_empty() {
                wrapped.push(String::new());
                continue;
            }

            let chars: Vec<char> = line.chars().collect();
            for chunk in chars.chunks(width) {
                wrapped.push(chunk.iter().collect());
            }
        }

        if wrapped.is_empty() {
            wrapped.push(String::new());
        }

        wrapped
    }

    fn apply_reflowed_lines(&mut self, columns: u16, rows: u16, wrapped: &[String]) {
        let rows_usize = rows.max(1) as usize;
        let split_at = wrapped.len().saturating_sub(rows_usize);
        let (scrollback, visible) = wrapped.split_at(split_at);

        self.scrollback = scrollback.iter().map(|line| Self::line_to_row(line, columns)).collect();

        self.buffer = visible.iter().map(|line| Self::line_to_row(line, columns)).collect();

        while self.buffer.len() < rows_usize {
            self.buffer.push(vec![Cell::default(); columns as usize]);
        }

        self.size = (columns, rows);
        self.cursor = (0, rows.saturating_sub(1));
        self.saved_cursor = None;
        self.pending_wrap = false;
    }

    fn line_to_row(line: &str, columns: u16) -> Vec<Cell> {
        let mut row: Vec<Cell> =
            line.chars().take(columns as usize).map(|ch| Cell::new(ch, Style::default())).collect();
        row.resize(columns as usize, Cell::default());
        row
    }

    /// Get all lines as a vector of strings (trailing whitespace trimmed)
    pub fn get_lines(&self) -> Vec<String> {
        self.buffer.iter().map(|cells| cells.iter().map(|c| c.ch).collect::<String>().trim_end().to_string()).collect()
    }

    /// Get full terminal transcript (scrollback history + visible buffer).
    pub fn get_transcript_lines(&self) -> Vec<String> {
        self.scrollback
            .iter()
            .chain(self.buffer.iter())
            .map(|cells| cells.iter().map(|c| c.ch).collect::<String>().trim_end().to_string())
            .collect()
    }

    /// Get current cursor position as (column, row).
    #[allow(dead_code)]
    pub fn cursor_position(&self) -> (u16, u16) {
        self.cursor
    }

    /// Get the style at a specific buffer position.
    pub fn get_style_at(&self, row: usize, col: usize) -> Style {
        self.buffer.get(row).and_then(|r| r.get(col)).map_or(Style::default(), |c| c.style)
    }

    /// Find the first occurrence of `text` on the given row and return its style.
    ///
    /// Returns the style of the first character of the matched text.
    pub fn style_of_text(&self, row: usize, text: &str) -> Option<Style> {
        let row_data = self.buffer.get(row)?;
        let row_text: String = row_data.iter().map(|c| c.ch).collect();
        let byte_offset = row_text.find(text)?;
        // Convert byte offset to character index
        let char_index = row_text[..byte_offset].chars().count();
        Some(row_data[char_index].style)
    }

    /// Clear the entire buffer
    pub fn clear(&mut self) {
        for row in &mut self.buffer {
            for cell in row {
                *cell = Cell::default();
            }
        }
    }

    /// Clear the current line
    pub fn clear_line(&mut self) {
        if let Some(row) = self.buffer.get_mut(self.cursor.1 as usize) {
            for cell in row {
                *cell = Cell::default();
            }
        }
    }

    /// Move cursor to absolute position
    pub fn move_to(&mut self, col: u16, row: u16) {
        self.cursor = (col.min(self.size.0.saturating_sub(1)), row.min(self.size.1.saturating_sub(1)));
        self.pending_wrap = false;
    }

    /// Move cursor to column (keep same row)
    pub fn move_to_column(&mut self, col: u16) {
        self.cursor.0 = col.min(self.size.0.saturating_sub(1));
        self.pending_wrap = false;
    }

    /// Move cursor left by n positions
    pub fn move_left(&mut self, n: u16) {
        self.cursor.0 = self.cursor.0.saturating_sub(n);
        self.pending_wrap = false;
    }

    /// Move cursor right by n positions
    pub fn move_right(&mut self, n: u16) {
        self.cursor.0 = (self.cursor.0 + n).min(self.size.0.saturating_sub(1));
        self.pending_wrap = false;
    }

    /// Write a single character at current cursor position and advance cursor
    fn write_char(&mut self, ch: char) {
        match ch {
            '\n' => {
                self.pending_wrap = false;
                if self.cursor.1 >= self.size.1.saturating_sub(1) {
                    let removed = self.buffer.remove(0);
                    self.scrollback.push(removed);
                    self.buffer.push(vec![Cell::default(); self.size.0 as usize]);
                } else {
                    self.cursor.1 += 1;
                }
                self.cursor.0 = 0;
            }
            '\r' => {
                self.cursor.0 = 0;
                self.pending_wrap = false;
            }
            '\t' => {
                for _ in 0..4 {
                    self.write_char_at_cursor(' ');
                }
            }
            _ => {
                self.write_char_at_cursor(ch);
            }
        }
    }

    /// Write a character at the current cursor position (delayed wrap).
    ///
    /// When the cursor is at the last column with `pending_wrap` set,
    /// the next printable character triggers the wrap first.
    fn write_char_at_cursor(&mut self, ch: char) {
        if self.pending_wrap {
            self.pending_wrap = false;
            self.cursor.0 = 0;
            if self.cursor.1 >= self.size.1.saturating_sub(1) {
                let removed = self.buffer.remove(0);
                self.scrollback.push(removed);
                self.buffer.push(vec![Cell::default(); self.size.0 as usize]);
            } else {
                self.cursor.1 += 1;
            }
        }

        if let Some(row) = self.buffer.get_mut(self.cursor.1 as usize)
            && let Some(cell) = row.get_mut(self.cursor.0 as usize)
        {
            *cell = Cell::new(ch, self.current_style);
            self.cursor.0 += 1;
            if self.cursor.0 >= self.size.0 {
                self.cursor.0 = self.size.0 - 1;
                self.pending_wrap = true;
            }
        }
    }

    /// Process a byte slice, handling ANSI escape sequences
    fn process_bytes(&mut self, buf: &[u8]) {
        let s = String::from_utf8_lossy(buf);
        let mut chars = s.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '\x1b' {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    self.process_csi_sequence(&mut chars);
                } else if chars.peek() == Some(&'7') {
                    chars.next();
                    self.saved_cursor = Some(self.cursor);
                } else if chars.peek() == Some(&'8') {
                    chars.next();
                    if let Some(saved) = self.saved_cursor {
                        self.cursor = saved;
                    }
                }
            } else if ch == char::from(BELL) {
                self.bell_count += 1;
            } else {
                self.write_char(ch);
            }
        }
    }

    /// Process a CSI (Control Sequence Introducer) escape sequence
    #[allow(clippy::too_many_lines)]
    fn process_csi_sequence(&mut self, chars: &mut std::iter::Peekable<std::str::Chars>) {
        let private_mode = if chars.peek() == Some(&'?') {
            chars.next();
            true
        } else {
            false
        };

        let mut params = String::new();

        while let Some(&ch) = chars.peek() {
            if ch.is_ascii_digit() || ch == ';' || ch == ':' {
                params.push(ch);
                chars.next();
            } else {
                break;
            }
        }

        if private_mode {
            chars.next();
            return;
        }

        if let Some(cmd) = chars.next() {
            match cmd {
                'H' | 'f' => {
                    let parts: Vec<u16> = params.split(';').filter_map(|s| s.parse().ok()).collect();
                    let row = parts.first().copied().unwrap_or(1).saturating_sub(1);
                    let col = parts.get(1).copied().unwrap_or(1).saturating_sub(1);
                    self.move_to(col, row);
                }
                'A' => {
                    let n = params.parse().unwrap_or(1);
                    self.cursor.1 = self.cursor.1.saturating_sub(n);
                    self.pending_wrap = false;
                }
                'B' => {
                    let n = params.parse().unwrap_or(1);
                    self.cursor.1 = (self.cursor.1 + n).min(self.size.1.saturating_sub(1));
                    self.pending_wrap = false;
                }
                'C' => {
                    let n = params.parse().unwrap_or(1);
                    self.move_right(n);
                }
                'D' => {
                    let n = params.parse().unwrap_or(1);
                    self.move_left(n);
                }
                'G' => {
                    let col = params.parse::<u16>().unwrap_or(1).saturating_sub(1);
                    self.move_to_column(col);
                }
                'J' => {
                    let n = params.parse().unwrap_or(0);
                    match n {
                        0 => {
                            for row in self.cursor.1..self.size.1 {
                                if let Some(r) = self.buffer.get_mut(row as usize) {
                                    let start = if row == self.cursor.1 { self.cursor.0 as usize } else { 0 };
                                    for cell in r.iter_mut().skip(start) {
                                        *cell = Cell::default();
                                    }
                                }
                            }
                        }
                        2 => {
                            self.clear();
                        }
                        3 => {
                            self.scrollback.clear();
                        }
                        _ => {}
                    }
                }
                'K' => {
                    let n = params.parse().unwrap_or(0);
                    match n {
                        0 => {
                            if let Some(row) = self.buffer.get_mut(self.cursor.1 as usize) {
                                for cell in row.iter_mut().skip(self.cursor.0 as usize) {
                                    *cell = Cell::default();
                                }
                            }
                        }
                        2 => {
                            self.clear_line();
                        }
                        _ => {}
                    }
                }
                's' => {
                    self.saved_cursor = Some(self.cursor);
                }
                'u' => {
                    if let Some(saved) = self.saved_cursor {
                        self.cursor = saved;
                        self.pending_wrap = false;
                    }
                }
                'm' => {
                    self.apply_sgr(&params);
                }
                _ => {}
            }
        }
    }

    /// Apply SGR (Select Graphic Rendition) parameters to update `current_style`.
    #[allow(clippy::cast_possible_truncation)]
    fn apply_sgr(&mut self, params: &str) {
        if params.is_empty() {
            self.current_style = Style::default();
            return;
        }

        // Split on ';' for parameter groups, then take the first colon-delimited
        // sub-parameter as the primary code (e.g. "4:1" → 4 for underline style).
        let codes: Vec<u16> = params
            .split(';')
            .filter_map(|s| {
                let primary = s.split(':').next().unwrap_or(s);
                primary.parse().ok()
            })
            .collect();
        let mut i = 0;
        while i < codes.len() {
            match codes[i] {
                0 => self.current_style = Style::default(),
                1 => self.current_style.bold = true,
                2 => self.current_style.dim = true,
                3 => self.current_style.italic = true,
                4 => self.current_style.underline = true,
                9 => self.current_style.strikethrough = true,
                22 => {
                    self.current_style.bold = false;
                    self.current_style.dim = false;
                }
                23 => self.current_style.italic = false,
                24 => self.current_style.underline = false,
                29 => self.current_style.strikethrough = false,
                30..=37 => {
                    self.current_style.fg = Some(standard_color(codes[i] as u8 - 30));
                }
                38 => {
                    i += 1;
                    if i < codes.len() {
                        match codes[i] {
                            5 if i + 1 < codes.len() => {
                                i += 1;
                                self.current_style.fg = Some(Color::AnsiValue(codes[i] as u8));
                            }
                            2 if i + 3 < codes.len() => {
                                self.current_style.fg = Some(Color::Rgb {
                                    r: codes[i + 1] as u8,
                                    g: codes[i + 2] as u8,
                                    b: codes[i + 3] as u8,
                                });
                                i += 3;
                            }
                            _ => {}
                        }
                    }
                }
                39 => self.current_style.fg = None,
                40..=47 => {
                    self.current_style.bg = Some(standard_color(codes[i] as u8 - 40));
                }
                48 => {
                    i += 1;
                    if i < codes.len() {
                        match codes[i] {
                            5 if i + 1 < codes.len() => {
                                i += 1;
                                self.current_style.bg = Some(Color::AnsiValue(codes[i] as u8));
                            }
                            2 if i + 3 < codes.len() => {
                                self.current_style.bg = Some(Color::Rgb {
                                    r: codes[i + 1] as u8,
                                    g: codes[i + 2] as u8,
                                    b: codes[i + 3] as u8,
                                });
                                i += 3;
                            }
                            _ => {}
                        }
                    }
                }
                49 => self.current_style.bg = None,
                90..=97 => {
                    self.current_style.fg = Some(bright_color(codes[i] as u8 - 90));
                }
                100..=107 => {
                    self.current_style.bg = Some(bright_color(codes[i] as u8 - 100));
                }
                _ => {}
            }
            i += 1;
        }
    }
}

/// Map ANSI standard color index (0-7) to crossterm Color.
fn standard_color(index: u8) -> Color {
    match index {
        0 => Color::Black,
        1 => Color::DarkRed,
        2 => Color::DarkGreen,
        3 => Color::DarkYellow,
        4 => Color::DarkBlue,
        5 => Color::DarkMagenta,
        6 => Color::DarkCyan,
        7 => Color::Grey,
        _ => Color::Reset,
    }
}

/// Map ANSI bright color index (0-7) to crossterm Color.
fn bright_color(index: u8) -> Color {
    match index {
        0 => Color::DarkGrey,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::White,
        _ => Color::Reset,
    }
}

impl Write for TestTerminal {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.escape_buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.escape_buffer.is_empty() {
            let bytes = std::mem::take(&mut self.escape_buffer);
            self.process_bytes(&bytes);
        }
        Ok(())
    }
}

/// Asserts a test terminal buffer matches the expected output.
/// Each element of the expected vector represents a row.
/// Trailing whitespace is ignored on each line.
pub fn assert_buffer_eq<S: AsRef<str>>(terminal: &TestTerminal, expected: &[S]) {
    let actual_lines = terminal.get_lines();
    let max_lines = expected.len().max(actual_lines.len());

    for i in 0..max_lines {
        let expected_line = expected.get(i).map_or("", AsRef::as_ref);
        let actual_line = actual_lines.get(i).map_or("", String::as_str);

        assert_eq!(
            actual_line,
            expected_line,
            "Line {i} mismatch:\n  Expected: '{expected_line}'\n  Got:      '{actual_line}'\n\nFull buffer:\n{}",
            actual_lines.join("\n")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_write() {
        let mut term = TestTerminal::new(80, 24);
        write!(term, "Hello").unwrap();
        term.flush().unwrap();
        let lines = term.get_lines();
        assert_eq!(lines[0], "Hello");
    }

    #[test]
    fn test_newline() {
        let mut term = TestTerminal::new(80, 24);
        write!(term, "Line 1\nLine 2").unwrap();
        term.flush().unwrap();
        assert_buffer_eq(&term, &["Line 1", "Line 2"]);
    }

    #[test]
    fn test_carriage_return() {
        let mut term = TestTerminal::new(80, 24);
        write!(term, "Hello\rWorld").unwrap();
        term.flush().unwrap();
        let lines = term.get_lines();
        assert_eq!(lines[0], "World");
    }

    #[test]
    fn test_ansi_cursor_position() {
        let mut term = TestTerminal::new(80, 24);
        write!(term, "\x1b[3;5HX").unwrap();
        term.flush().unwrap();
        let lines = term.get_lines();
        assert_eq!(&lines[2][4..5], "X");
    }

    #[test]
    fn test_ansi_clear_line() {
        let mut term = TestTerminal::new(80, 24);
        write!(term, "Hello World").unwrap();
        write!(term, "\x1b[1G\x1b[K").unwrap();
        term.flush().unwrap();
        let lines = term.get_lines();
        assert_eq!(lines[0], "");
    }

    #[test]
    fn test_assert_buffer_eq() {
        let mut term = TestTerminal::new(80, 24);
        write!(term, "Line 1\nLine 2\nLine 3").unwrap();
        term.flush().unwrap();

        assert_buffer_eq(&term, &["Line 1", "Line 2", "Line 3"]);
    }

    #[test]
    #[should_panic(expected = "Line 0 mismatch")]
    fn test_assert_buffer_eq_fails() {
        let mut term = TestTerminal::new(80, 24);
        write!(term, "Wrong").unwrap();
        term.flush().unwrap();

        assert_buffer_eq(&term, &["Expected"]);
    }

    #[test]
    fn test_private_mode_sequences_ignored() {
        let mut term = TestTerminal::new(80, 24);
        write!(term, "\x1b[?2026hHello\x1b[?2026l").unwrap();
        term.flush().unwrap();
        let lines = term.get_lines();
        assert_eq!(lines[0], "Hello");
    }

    #[test]
    fn test_cursor_save_restore() {
        let mut term = TestTerminal::new(80, 24);

        write!(term, "\x1b[6;11HFirst").unwrap();
        write!(term, "\x1b7").unwrap();
        write!(term, "\x1b[1;1HSecond").unwrap();
        write!(term, "\x1b8Third").unwrap();

        term.flush().unwrap();

        let lines = term.get_lines();
        assert_eq!(lines[0], "Second");
        assert_eq!(lines[5], "          FirstThird");
    }

    #[test]
    fn test_transcript_includes_scrolled_off_lines() {
        let mut term = TestTerminal::new(6, 2);
        write!(term, "L1\nL2\nL3").unwrap();
        term.flush().unwrap();

        let visible = term.get_lines();
        assert_eq!(visible[0], "L2");
        assert_eq!(visible[1], "L3");

        let transcript = term.get_transcript_lines();
        assert_eq!(transcript, vec!["L1", "L2", "L3"]);
    }

    #[test]
    fn test_sgr_bold() {
        let mut term = TestTerminal::new(80, 24);
        write!(term, "\x1b[1mbold\x1b[0m").unwrap();
        term.flush().unwrap();
        let lines = term.get_lines();
        assert_eq!(lines[0], "bold");
        assert!(term.get_style_at(0, 0).bold);
        assert!(!term.get_style_at(0, 4).bold);
    }

    #[test]
    fn test_sgr_fg_color() {
        let mut term = TestTerminal::new(80, 24);
        write!(term, "\x1b[31mred\x1b[0m").unwrap();
        term.flush().unwrap();
        assert_eq!(term.get_style_at(0, 0).fg, Some(Color::DarkRed));
        assert_eq!(term.get_style_at(0, 3).fg, None);
    }

    #[test]
    fn test_sgr_rgb_color() {
        let mut term = TestTerminal::new(80, 24);
        write!(term, "\x1b[38;2;255;128;0mrgb\x1b[0m").unwrap();
        term.flush().unwrap();
        assert_eq!(term.get_style_at(0, 0).fg, Some(Color::Rgb { r: 255, g: 128, b: 0 }));
    }

    #[test]
    fn test_style_of_text() {
        let mut term = TestTerminal::new(80, 24);
        write!(term, "plain \x1b[1mbold\x1b[0m rest").unwrap();
        term.flush().unwrap();
        let style = term.style_of_text(0, "bold").unwrap();
        assert!(style.bold);
        let style = term.style_of_text(0, "plain").unwrap();
        assert!(!style.bold);
    }
}
