use serde::Serialize;

/// Outcome of running a collection of eval files.
#[derive(Debug, Clone, Serialize)]
pub struct EvalFilesReport {
    pub evals: Vec<EvalOutcome>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvalOutcome {
    pub name: String,
    pub passed: bool,
    pub failures: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge: Option<JudgeSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_context: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JudgeSummary {
    pub passed: bool,
    pub score: f64,
    pub reason: String,
    pub criteria: Vec<JudgeCriterionSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JudgeCriterionSummary {
    pub id: String,
    pub description: String,
    pub blocking: bool,
    pub weight: f64,
    pub threshold: f64,
    pub score: f64,
    pub reason: String,
}

impl EvalFilesReport {
    pub fn passed(&self) -> bool {
        self.evals.iter().all(|eval| eval.passed)
    }

    pub fn passed_count(&self) -> usize {
        self.evals.iter().filter(|eval| eval.passed).count()
    }

    pub fn failed_count(&self) -> usize {
        self.evals.iter().filter(|eval| !eval.passed).count()
    }
}

impl JudgeSummary {
    /// Failure messages for blocking criteria that scored below their threshold.
    pub fn blocking_failures(&self) -> impl Iterator<Item = String> + '_ {
        self.criteria
            .iter()
            .filter(|criterion| criterion.blocking && !criterion.passed())
            .map(|criterion| format!("judge criterion `{}`: {}", criterion.id, criterion.reason))
    }
}

impl JudgeCriterionSummary {
    pub fn passed(&self) -> bool {
        self.score >= self.threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocking_failures_report_only_blocking_criteria_below_threshold() {
        let criterion = |id: &str, blocking, score| JudgeCriterionSummary {
            id: id.to_string(),
            description: "desc".to_string(),
            blocking,
            weight: 1.0,
            threshold: 0.8,
            score,
            reason: format!("{id} reason"),
        };
        let summary = JudgeSummary {
            passed: false,
            score: 0.0,
            reason: "r".to_string(),
            criteria: vec![
                criterion("met", true, 0.9),
                criterion("failed", true, 0.5),
                criterion("advisory", false, 0.0),
            ],
        };

        let failures: Vec<String> = summary.blocking_failures().collect();

        assert_eq!(failures, vec!["judge criterion `failed`: failed reason".to_string()]);
    }
}
