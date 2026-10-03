use crate::temp_dir::TempDir;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolResultTruncator {
    pub head: usize,
    pub tail: usize,
}

impl ToolResultTruncator {
    pub fn truncate(&self, text: String, dir: &TempDir, label: &str) -> String {
        let Some((head, tail)) = self.split(&text) else {
            return text;
        };
        let saved = match dir.save(label, &text) {
            Ok(path) => format!("{SAVED_TO}{}", path.display()),
            Err(error) => {
                tracing::warn!("Failed to save oversized output: {error}");
                "the full output could not be saved".to_string()
            }
        };
        let omitted = text.len() - head.len() - tail.len();
        format!("{head}\n[... {omitted} bytes omitted; {saved}{MARKER_END}{tail}")
    }

    fn split<'a>(&self, text: &'a str) -> Option<(&'a str, &'a str)> {
        (text.len() > self.head + self.tail).then(|| {
            let head = &text[..text.floor_char_boundary(self.head)];
            let tail = &text[text.ceil_char_boundary(text.len() - self.tail)..];
            (head, tail)
        })
    }
}

pub fn saved_path(excerpt: &str) -> Option<&Path> {
    let (_, rest) = excerpt.split_once(SAVED_TO)?;
    rest.split_once(MARKER_END).map(|(path, _)| Path::new(path))
}

const SAVED_TO: &str = "full output saved to ";
const MARKER_END: &str = " ...]\n";

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::read_to_string;

    const TRUNCATOR: ToolResultTruncator = ToolResultTruncator { head: 16, tail: 12 };

    fn truncated(text: &str) -> (String, TempDir) {
        let dir = TempDir::new();
        (TRUNCATOR.truncate(text.to_string(), &dir, "out"), dir)
    }

    fn head_and_tail(excerpt: &str) -> (&str, &str) {
        let (head, rest) = excerpt.split_once("\n[... ").unwrap();
        (head, rest.split_once(MARKER_END).unwrap().1)
    }

    #[test]
    fn text_is_truncated_once_it_outgrows_the_head_and_tail() {
        let (fits, _dir) = truncated(&"x".repeat(28));
        assert_eq!(fits, "x".repeat(28));
        assert_eq!(saved_path(&fits), None);

        let (excerpt, _dir) = truncated(&"x".repeat(29));
        assert_eq!(head_and_tail(&excerpt), ("x".repeat(16).as_str(), "x".repeat(12).as_str()));
    }

    #[test]
    fn truncated_text_keeps_byte_limited_excerpts_and_saves_the_full_text() {
        let text = (1..=20).map(|n| format!("line {n}\n")).collect::<Vec<_>>().concat();

        let (excerpt, _dir) = truncated(&text);

        let saved = saved_path(&excerpt).unwrap();
        let head = "line 1\nline 2\nli";
        let tail = " 19\nline 20\n";
        let omitted = text.len() - head.len() - tail.len();
        let marker = format!("[... {omitted} bytes omitted; full output saved to {} ...]", saved.display());
        assert_eq!(excerpt, format!("{head}\n{marker}\n{tail}"));
        assert_eq!(read_to_string(saved).unwrap(), text);
    }

    #[test]
    fn truncate_keeps_the_final_line_even_when_it_overflows_the_tail() {
        let (excerpt, _dir) = truncated(&format!("short\n{}\n", "x".repeat(50)));
        assert_eq!(head_and_tail(&excerpt).1, format!("{}\n", "x".repeat(11)));
    }

    #[test]
    fn excerpts_stay_within_their_budgets_at_utf8_boundaries() {
        let (excerpt, _dir) = truncated(&"🦀".repeat(20));
        assert_eq!(head_and_tail(&excerpt), ("🦀".repeat(4).as_str(), "🦀".repeat(3).as_str()));

        let (excerpt, _dir) = truncated(&"─".repeat(20));
        assert_eq!(head_and_tail(&excerpt), ("─".repeat(5).as_str(), "─".repeat(4).as_str()));
    }
}
