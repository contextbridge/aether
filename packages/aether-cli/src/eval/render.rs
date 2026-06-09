use crate::output::OutputFormat;
use aether_evals::{EvalFilesReport, EvalOutcome};
use std::fmt::Write as _;

pub fn render(report: &EvalFilesReport, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => serde_json::to_string(report).expect("EvalFilesReport serializes"),
        OutputFormat::Pretty => serde_json::to_string_pretty(report).expect("EvalFilesReport serializes"),
        OutputFormat::Text => {
            let mut output = String::new();
            for eval in &report.evals {
                if eval.passed {
                    let _ = writeln!(output, "✓ {}{}", eval.name, judge_score(eval));
                    continue;
                }

                let _ = writeln!(output, "✗ {}{}", eval.name, judge_score(eval));
                for failure in &eval.failures {
                    let _ = writeln!(output, "    - {}", failure.replace('\n', "\n      "));
                }
                for line in eval.failure_context.iter().flat_map(|context| context.lines()) {
                    let _ = writeln!(output, "    {line}");
                }
            }

            let _ = write!(output, "\n{} passed, {} failed", report.passed_count(), report.failed_count());
            output
        }
    }
}

fn judge_score(eval: &EvalOutcome) -> String {
    eval.judge.as_ref().map(|judge| format!(" (judge score {:.0}%)", judge.score * 100.0)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_evals::{EvalOutcome, JudgeCriterionSummary, JudgeSummary};

    fn report() -> EvalFilesReport {
        EvalFilesReport {
            evals: vec![
                EvalOutcome {
                    name: "passing".to_string(),
                    passed: true,
                    failures: vec![],
                    judge: Some(JudgeSummary {
                        passed: true,
                        score: 1.0,
                        reason: "weighted score 1.00; all blockers met".to_string(),
                        criteria: vec![JudgeCriterionSummary {
                            id: "behavior".to_string(),
                            description: "correct behavior".to_string(),
                            blocking: true,
                            weight: 1.0,
                            threshold: 1.0,
                            score: 1.0,
                            reason: "correct".to_string(),
                        }],
                    }),
                    failure_context: None,
                },
                EvalOutcome {
                    name: "failing".to_string(),
                    passed: false,
                    failures: vec!["expected tool `bash` to be called".to_string()],
                    judge: Some(JudgeSummary {
                        passed: false,
                        score: 0.0,
                        reason: "weighted score 0.50; one or more blockers failed".to_string(),
                        criteria: vec![JudgeCriterionSummary {
                            id: "behavior".to_string(),
                            description: "correct behavior".to_string(),
                            blocking: true,
                            weight: 1.0,
                            threshold: 1.0,
                            score: 0.5,
                            reason: "wrong".to_string(),
                        }],
                    }),
                    failure_context: Some("Eval failure context\nWorkspace: /tmp/x".to_string()),
                },
            ],
        }
    }

    #[test]
    fn text_render_marks_pass_fail_and_totals() {
        let text = render(&report(), OutputFormat::Text);

        assert!(text.contains("✓ passing (judge score 100%)"));
        assert!(text.contains("✗ failing (judge score 0%)"));
        assert!(text.contains("expected tool `bash` to be called"));
        assert!(text.contains("1 passed, 1 failed"));
    }

    #[test]
    fn json_render_is_machine_readable() {
        let json = render(&report(), OutputFormat::Json);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["evals"][0]["name"], "passing");
        assert_eq!(parsed["evals"][1]["passed"], false);
        assert_eq!(parsed["evals"][1]["failures"][0], "expected tool `bash` to be called");
        assert_eq!(parsed["evals"][0]["judge"]["score"], 1.0);
        assert_eq!(parsed["evals"][1]["judge"]["criteria"][0]["id"], "behavior");
        assert!(parsed["evals"][1]["judge"]["criteria"][0]["passed"].is_null());
    }
}
