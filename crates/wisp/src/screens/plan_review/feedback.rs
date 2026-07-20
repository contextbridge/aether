use super::document::PlanDocument;
use std::fmt::Write;

pub struct ReviewComment {
    pub line_no: usize,
    pub body: String,
}

impl ReviewComment {
    pub fn new(line_no: usize, body: String) -> Self {
        Self { line_no, body }
    }
}

pub fn compile_feedback(document: &PlanDocument, comments: &[ReviewComment]) -> String {
    if comments.is_empty() {
        return "Plan needs changes, but no inline comments were provided.".to_string();
    }

    let mut output = String::from("# Plan review feedback\n\n");
    let mut current_section: Option<usize> = None;

    for comment in comments {
        let Some(line) = document.line_by_no(comment.line_no) else {
            continue;
        };

        if line.section_index != current_section {
            if let Some(section_title) = document.section_title_for(line) {
                let _ = writeln!(output, "## {section_title}");
                output.push('\n');
            }
            current_section = line.section_index;
        }

        let _ = writeln!(output, "### Line {}", line.line_no);
        if !line.text.trim().is_empty() {
            let _ = writeln!(output, "`{}`", sanitize_line_snippet(&line.text));
        }

        let mut wrote_point = false;
        for feedback_line in comment.body.lines().map(str::trim).filter(|line| !line.is_empty()) {
            let _ = writeln!(output, "- {feedback_line}");
            wrote_point = true;
        }

        if !wrote_point {
            output.push_str("- (no comment text provided)\n");
        }

        output.push('\n');
    }

    if output.trim() == "# Plan review feedback" {
        "Plan needs changes, but no inline comments were provided.".to_string()
    } else {
        output.trim().to_string()
    }
}

fn sanitize_line_snippet(line: &str) -> String {
    let mut trimmed = line.trim().replace('`', "\\`");
    if trimmed.chars().count() > 140 {
        trimmed = trimmed.chars().take(137).collect::<String>() + "...";
    }
    trimmed
}

#[cfg(test)]
mod tests {
    use super::super::document::PlanDocument;
    use super::*;

    #[test]
    fn compile_feedback_falls_back_when_no_comments() {
        let document = PlanDocument::parse("/tmp/plan.md", "# Plan");
        let feedback = compile_feedback(&document, &[]);
        assert!(feedback.contains("no inline comments"));
    }

    #[test]
    fn compile_feedback_includes_line_numbers_and_comments() {
        let document = PlanDocument::parse("/tmp/plan.md", "# Overview\nline");
        let comments = vec![ReviewComment::new(2, "Please expand this".to_string())];

        let feedback = compile_feedback(&document, &comments);
        assert!(feedback.contains("Line 2"));
        assert!(feedback.contains("Please expand this"));
    }

    #[test]
    fn compile_feedback_groups_by_section() {
        let document = PlanDocument::parse("/tmp/plan.md", "# Intro\nline1\n## Details\nline3");
        let comments =
            vec![ReviewComment::new(2, "fix intro".to_string()), ReviewComment::new(4, "fix details".to_string())];

        let feedback = compile_feedback(&document, &comments);
        assert!(feedback.contains("## Intro"));
        assert!(feedback.contains("## Details"));
        assert!(feedback.contains("fix intro"));
        assert!(feedback.contains("fix details"));
    }

    #[test]
    fn compile_feedback_handles_multiline_comments() {
        let document = PlanDocument::parse("/tmp/plan.md", "# Top\nline");
        let comments = vec![ReviewComment::new(2, "First point\nSecond point".to_string())];

        let feedback = compile_feedback(&document, &comments);
        assert!(feedback.contains("- First point"));
        assert!(feedback.contains("- Second point"));
    }

    #[test]
    fn compile_feedback_sanitizes_backticks_in_snippets() {
        let document = PlanDocument::parse("/tmp/plan.md", "# Top\nuse `backtick` here");
        let comments = vec![ReviewComment::new(2, "ok".to_string())];

        let feedback = compile_feedback(&document, &comments);
        assert!(feedback.contains("\\`backtick\\`"));
    }

    #[test]
    fn compile_feedback_truncates_long_snippets() {
        let long_line = "x".repeat(200);
        let markdown = format!("# Top\n{long_line}");
        let document = PlanDocument::parse("/tmp/plan.md", &markdown);
        let comments = vec![ReviewComment::new(2, "ok".to_string())];

        let feedback = compile_feedback(&document, &comments);
        assert!(!feedback.contains(&long_line));
        assert!(feedback.contains("..."));
    }

    #[test]
    fn sanitize_handles_empty_line() {
        assert_eq!(sanitize_line_snippet(""), "");
    }

    #[test]
    fn compile_feedback_handles_blank_source_lines() {
        let document = PlanDocument::parse("/tmp/plan.md", "# Top\n\n\ntext");
        let comments = vec![ReviewComment::new(2, "blank line above".to_string())];

        let feedback = compile_feedback(&document, &comments);
        assert!(feedback.contains("Line 2"));
        assert!(feedback.contains("blank line above"));
    }
}
