#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineDocument {
    lines: Vec<String>,
    line_endings: Vec<Option<String>>,
    default_line_ending: String,
}

impl LineDocument {
    pub fn parse(content: &str) -> Self {
        let (lines, line_endings) = parse_lines(content);
        let default_line_ending = detect_default_line_ending(&line_endings).unwrap_or("\n").to_string();

        Self { lines, line_endings, default_line_ending }
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn replace_range(&mut self, start: usize, end: usize, replacement_lines: Vec<String>) {
        if start == end {
            self.insert_lines(start, replacement_lines);
        } else {
            self.replace_lines(start, end, replacement_lines);
        }
    }

    pub fn join(&self) -> String {
        self.lines.iter().zip(&self.line_endings).fold(String::new(), |mut content, (line, ending)| {
            content.push_str(line);
            if let Some(ending) = ending {
                content.push_str(ending);
            }
            content
        })
    }

    fn insert_lines(&mut self, index: usize, replacement_lines: Vec<String>) {
        if replacement_lines.is_empty() {
            return;
        }

        if index > 0 && self.line_endings[index - 1].is_none() {
            self.line_endings[index - 1] = Some(self.default_line_ending.clone());
        }

        let inserted_endings = self.inserted_line_endings(replacement_lines.len(), index < self.lines.len());
        self.lines.splice(index..index, replacement_lines);
        self.line_endings.splice(index..index, inserted_endings);
    }

    fn replace_lines(&mut self, start: usize, end: usize, replacement_lines: Vec<String>) {
        let removed_final_ending = self.line_endings[end - 1].clone();

        if replacement_lines.is_empty() {
            self.lines.splice(start..end, []);
            self.line_endings.splice(start..end, []);
            if start == self.lines.len() && start > 0 && removed_final_ending.is_none() {
                self.line_endings[start - 1] = None;
            }
            return;
        }

        let replacement_endings = self.replacement_line_endings(replacement_lines.len(), removed_final_ending.as_ref());
        self.lines.splice(start..end, replacement_lines);
        self.line_endings.splice(start..end, replacement_endings);
    }

    fn inserted_line_endings(&self, line_count: usize, has_following_line: bool) -> Vec<Option<String>> {
        (0..line_count)
            .map(|index| {
                if has_following_line || index + 1 < line_count { Some(self.default_line_ending.clone()) } else { None }
            })
            .collect()
    }

    fn replacement_line_endings(&self, line_count: usize, final_ending: Option<&String>) -> Vec<Option<String>> {
        (0..line_count)
            .map(
                |index| {
                    if index + 1 == line_count { final_ending.cloned() } else { Some(self.default_line_ending.clone()) }
                },
            )
            .collect()
    }
}

fn parse_lines(content: &str) -> (Vec<String>, Vec<Option<String>>) {
    let mut lines = Vec::new();
    let mut endings = Vec::new();
    let bytes = content.as_bytes();
    let mut line_start = 0;
    let mut index = 0;

    while index < bytes.len() {
        let ending = match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => Some(("\r\n", 2)),
            b'\r' => Some(("\r", 1)),
            b'\n' => Some(("\n", 1)),
            _ => None,
        };

        if let Some((ending, ending_len)) = ending {
            lines.push(content[line_start..index].to_string());
            endings.push(Some(ending.to_string()));
            index += ending_len;
            line_start = index;
        } else {
            index += 1;
        }
    }

    if line_start < content.len() {
        lines.push(content[line_start..].to_string());
        endings.push(None);
    }

    (lines, endings)
}

fn detect_default_line_ending(line_endings: &[Option<String>]) -> Option<&str> {
    line_endings.iter().flatten().map(String::as_str).next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_returns_logical_lines_without_phantom_trailing_line() {
        let document = LineDocument::parse("one\ntwo\n");

        assert_eq!(document.lines(), &["one".to_string(), "two".to_string()]);
        assert_eq!(document.line_count(), 2);
        assert_eq!(document.join(), "one\ntwo\n");
    }

    #[test]
    fn join_preserves_existing_line_endings() {
        for (content, expected) in [
            ("one\ntwo", "one\ntwo"),
            ("one\r\ntwo\r\n", "one\r\ntwo\r\n"),
            ("one\rtwo\r", "one\rtwo\r"),
            ("one\r\ntwo\nthree\r", "one\r\ntwo\nthree\r"),
        ] {
            assert_eq!(LineDocument::parse(content).join(), expected);
        }
    }

    #[test]
    fn empty_content_has_no_lines() {
        let document = LineDocument::parse("");

        assert!(document.lines().is_empty());
        assert_eq!(document.join(), "");
    }

    #[test]
    fn replace_range_preserves_removed_line_final_ending() {
        let mut document = LineDocument::parse("one\r\ntwo\nthree");

        document.replace_range(1, 2, vec!["TWO".to_string()]);

        assert_eq!(document.join(), "one\r\nTWO\nthree");
    }

    #[test]
    fn insert_at_end_adds_separator_after_previous_final_line() {
        let mut document = LineDocument::parse("one\ntwo");

        document.replace_range(2, 2, vec!["three".to_string(), "four".to_string()]);

        assert_eq!(document.join(), "one\ntwo\nthree\nfour");
    }

    #[test]
    fn deleting_final_line_removes_previous_separator_when_file_had_no_final_newline() {
        let mut document = LineDocument::parse("one\ntwo");

        document.replace_range(1, 2, Vec::new());

        assert_eq!(document.join(), "one");
    }
}
