pub mod common;
pub mod diagnostics;
pub mod error;
pub mod registry;
pub mod render;
pub mod tools;

pub use registry::LspRegistry;

#[cfg(test)]
pub(crate) mod test_fixtures {
    use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, TextEdit, Uri};

    pub(crate) fn uri(path: &str) -> Uri {
        format!("file://{path}").parse().unwrap()
    }

    pub(crate) fn diagnostic(severity: DiagnosticSeverity, message: &str, line: u32) -> Diagnostic {
        Diagnostic {
            range: Range::new(Position::new(line, 0), Position::new(line, 10)),
            severity: Some(severity),
            code: Some(NumberOrString::String("E0308".to_string())),
            source: Some("rustc".to_string()),
            message: message.to_string(),
            ..Diagnostic::default()
        }
    }

    pub(crate) fn text_edit(line: u32, from: u32, to: u32, new_text: &str) -> TextEdit {
        TextEdit::new(Range::new(Position::new(line, from), Position::new(line, to)), new_text.to_string())
    }
}
