use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceMap<'a> {
    text: &'a str,
    line_starts: Vec<usize>,
}

impl<'a> SourceMap<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        let mut line_starts = vec![0];
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(index + 1);
            }
        }
        Self { text, line_starts }
    }

    pub(crate) fn text(&self) -> &'a str {
        self.text
    }

    pub(crate) fn line_no_for_offset(&self, offset: usize) -> usize {
        let clamped = offset.min(self.text.len());
        self.line_starts.partition_point(|line_start| *line_start <= clamped).max(1)
    }

    pub(crate) fn line_no_for_start(&self, range: &Range<usize>) -> usize {
        self.line_no_for_offset(range.start)
    }
}
