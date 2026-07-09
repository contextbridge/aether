use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    pub fn all() -> &'static [ReasoningEffort] {
        &[Self::Minimal, Self::Low, Self::Medium, Self::High, Self::Xhigh, Self::Max]
    }

    /// Cycles through only the given `levels`, wrapping to `None` after the last.
    /// Returns `None` when `levels` is empty.
    pub fn cycle_within(current: Option<Self>, levels: &[Self]) -> Option<Self> {
        if levels.is_empty() {
            return None;
        }
        match current {
            None => Some(levels[0]),
            Some(effort) => levels.iter().position(|&l| l == effort).and_then(|i| levels.get(i + 1)).copied(),
        }
    }

    /// Returns `self` if it's in `levels`, otherwise the highest level ≤ self.
    /// Falls back to the first element of `levels`. Panics if `levels` is empty.
    pub fn clamp_to(self, levels: &[Self]) -> Self {
        levels.iter().rev().find(|&&l| l <= self).copied().unwrap_or(*levels.first().expect("levels must not be empty"))
    }

    /// Converts `Option<ReasoningEffort>` to a config string value.
    pub fn config_str(effort: Option<Self>) -> &'static str {
        effort.map_or("none", Self::as_str)
    }

    /// Parse a string into an optional effort level.
    /// Accepts "none" / "" as `None`, and supported effort names as `Some`.
    pub fn parse(s: &str) -> Result<Option<Self>, String> {
        match s {
            "none" | "" => Ok(None),
            other => other.parse().map(Some),
        }
    }
}

impl fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ReasoningEffort {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            _ => Err(format!("Unknown reasoning effort: '{s}'")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_roundtrip() {
        for effort in ReasoningEffort::all() {
            let s = effort.to_string();
            let parsed: ReasoningEffort = s.parse().unwrap();
            assert_eq!(*effort, parsed);
        }
    }

    #[test]
    fn as_str_matches_display() {
        for effort in ReasoningEffort::all() {
            assert_eq!(effort.as_str(), effort.to_string());
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!("extreme".parse::<ReasoningEffort>().is_err());
    }

    #[test]
    fn all_returns_six_variants() {
        assert_eq!(ReasoningEffort::all().len(), 6);
    }

    #[test]
    fn parse_none_and_empty() {
        assert_eq!(ReasoningEffort::parse("none").unwrap(), None);
        assert_eq!(ReasoningEffort::parse("").unwrap(), None);
    }

    #[test]
    fn parse_valid_levels() {
        assert_eq!(ReasoningEffort::parse("high").unwrap(), Some(ReasoningEffort::High));
        assert_eq!(ReasoningEffort::parse("low").unwrap(), Some(ReasoningEffort::Low));
    }

    #[test]
    fn parse_rejects_unknown() {
        assert!(ReasoningEffort::parse("extreme").is_err());
    }

    #[test]
    fn config_str_values() {
        assert_eq!(ReasoningEffort::config_str(None), "none");
        assert_eq!(ReasoningEffort::config_str(Some(ReasoningEffort::Low)), "low");
        assert_eq!(ReasoningEffort::config_str(Some(ReasoningEffort::High)), "high");
    }

    #[test]
    fn serialize_produces_lowercase() {
        for effort in ReasoningEffort::all() {
            let json = serde_json::to_value(effort).unwrap();
            assert_eq!(json.as_str().unwrap(), effort.as_str());
        }
    }

    #[test]
    fn variants_are_ordered_by_effort() {
        let mut sorted = ReasoningEffort::all().to_vec();
        sorted.sort();
        assert_eq!(sorted, ReasoningEffort::all());
        assert!(ReasoningEffort::Minimal < ReasoningEffort::Low);
        assert!(ReasoningEffort::Xhigh < ReasoningEffort::Max);
    }

    #[test]
    fn cycle_within_three_levels() {
        use ReasoningEffort::*;
        let levels = &[Low, Medium, High];
        assert_eq!(ReasoningEffort::cycle_within(None, levels), Some(Low));
        assert_eq!(ReasoningEffort::cycle_within(Some(Low), levels), Some(Medium));
        assert_eq!(ReasoningEffort::cycle_within(Some(Medium), levels), Some(High));
        assert_eq!(ReasoningEffort::cycle_within(Some(High), levels), None);
    }

    #[test]
    fn cycle_within_five_levels() {
        use ReasoningEffort::*;
        let levels = &[Low, Medium, High, Xhigh, Max];
        assert_eq!(ReasoningEffort::cycle_within(None, levels), Some(Low));
        assert_eq!(ReasoningEffort::cycle_within(Some(High), levels), Some(Xhigh));
        assert_eq!(ReasoningEffort::cycle_within(Some(Xhigh), levels), Some(Max));
        assert_eq!(ReasoningEffort::cycle_within(Some(Max), levels), None);
    }

    #[test]
    fn cycle_within_empty_returns_none() {
        assert_eq!(ReasoningEffort::cycle_within(None, &[]), None);
        assert_eq!(ReasoningEffort::cycle_within(Some(ReasoningEffort::Low), &[]), None);
    }

    #[test]
    fn cycle_within_unknown_current_wraps_to_none() {
        use ReasoningEffort::*;
        // Current is Xhigh but levels only have Low/Medium/High
        assert_eq!(ReasoningEffort::cycle_within(Some(Xhigh), &[Low, Medium, High]), None);
    }

    #[test]
    fn clamp_to_self_in_levels() {
        use ReasoningEffort::*;
        assert_eq!(High.clamp_to(&[Low, Medium, High]), High);
        assert_eq!(Xhigh.clamp_to(&[Low, Medium, High, Xhigh]), Xhigh);
        assert_eq!(Max.clamp_to(&[Low, Medium, High, Xhigh, Max]), Max);
    }

    #[test]
    fn clamp_to_highest_le() {
        use ReasoningEffort::*;
        // Max not in [Low, Medium, High, Xhigh] -> clamp to Xhigh
        assert_eq!(Max.clamp_to(&[Low, Medium, High, Xhigh]), Xhigh);
    }

    #[test]
    fn clamp_to_fallback_first() {
        use ReasoningEffort::*;
        // Low not in [Medium, High] and no level ≤ Low → fallback to first (Medium)
        assert_eq!(Low.clamp_to(&[Medium, High]), Medium);
    }

    #[test]
    fn parse_extended_levels() {
        assert_eq!(ReasoningEffort::parse("minimal").unwrap(), Some(ReasoningEffort::Minimal));
        assert_eq!(ReasoningEffort::parse("xhigh").unwrap(), Some(ReasoningEffort::Xhigh));
        assert_eq!(ReasoningEffort::parse("max").unwrap(), Some(ReasoningEffort::Max));
        assert!(ReasoningEffort::parse("ultra").is_err());
    }
}
