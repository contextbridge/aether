use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(
    Default, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    #[default]
    Default,
    Disabled,
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
            Self::Default => "default",
            Self::Disabled => "disabled",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    pub fn all() -> &'static [ReasoningEffort] {
        &[Self::Default, Self::Disabled, Self::Minimal, Self::Low, Self::Medium, Self::High, Self::Xhigh, Self::Max]
    }

    pub fn selectable_levels() -> &'static [Self] {
        &[Self::Disabled, Self::Minimal, Self::Low, Self::Medium, Self::High, Self::Xhigh, Self::Max]
    }

    pub fn is_enabled(self) -> bool {
        matches!(self, Self::Minimal | Self::Low | Self::Medium | Self::High | Self::Xhigh | Self::Max)
    }

    /// Cycles through only the given `levels`, wrapping to `None` after the last.
    /// Returns `None` when `levels` is empty.
    pub fn cycle_within(current: Option<Self>, levels: &[Self]) -> Option<Self> {
        if levels.is_empty() {
            return None;
        }
        match current {
            None | Some(Self::Default) => Some(levels[0]),
            Some(effort) => levels.iter().position(|&l| l == effort).and_then(|i| levels.get(i + 1)).copied(),
        }
    }

    /// Cycles backwards through only the given `levels`, wrapping to `None` after the first.
    /// Returns `None` when `levels` is empty.
    pub fn cycle_within_back(current: Option<Self>, levels: &[Self]) -> Option<Self> {
        if levels.is_empty() {
            return None;
        }
        match current {
            None | Some(Self::Default) => Some(*levels.last().expect("levels is non-empty")),
            Some(effort) => levels
                .iter()
                .position(|&l| l == effort)
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| levels.get(i))
                .copied(),
        }
    }

    pub fn clamp_to(self, levels: &[Self]) -> Self {
        if !self.is_enabled() {
            return self;
        }
        levels
            .iter()
            .copied()
            .filter(|level| level.is_enabled() && *level <= self)
            .max()
            .or_else(|| levels.iter().copied().filter(|level| level.is_enabled()).min())
            .unwrap_or_default()
    }

    /// Converts `Option<ReasoningEffort>` to a config string value.
    pub fn config_str(effort: Option<Self>) -> &'static str {
        effort.unwrap_or_default().as_str()
    }

    /// Parse a string into an optional effort level.
    /// Empty input clears the setting; explicit selections, including default, are retained.
    pub fn parse(s: &str) -> Result<Option<Self>, String> {
        match s {
            "" => Ok(None),
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
            "default" => Ok(Self::Default),
            "disabled" => Ok(Self::Disabled),
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
    fn control_states_cycle_and_clamp_deliberately() {
        use ReasoningEffort::*;
        let levels = [Disabled, Low, High];
        assert_eq!(ReasoningEffort::cycle_within(Some(Default), &levels), Some(Disabled));
        assert_eq!(ReasoningEffort::cycle_within(Some(Disabled), &levels), Some(Low));
        assert_eq!(ReasoningEffort::cycle_within_back(Some(Default), &levels), Some(High));
        assert_eq!(ReasoningEffort::cycle_within_back(Some(Disabled), &levels), None);
        assert_eq!(Disabled.clamp_to(&[Low]), Disabled);
        assert_eq!(Default.clamp_to(&[Low]), Default);
        assert_eq!(Minimal.clamp_to(&[Disabled, Low]), Low);
        assert_eq!(High.clamp_to(&[Disabled]), Default);
        assert!(!Default.is_enabled());
        assert!(!Disabled.is_enabled());
        assert!(Minimal.is_enabled());
    }

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
    fn all_returns_eight_variants() {
        assert_eq!(ReasoningEffort::all().len(), 8);
    }

    #[test]
    fn parse_none_and_empty() {
        assert!(ReasoningEffort::parse("none").is_err());
        assert_eq!(ReasoningEffort::parse("").unwrap(), None);
    }

    #[test]
    fn parse_valid_levels() {
        assert_eq!(ReasoningEffort::parse("default").unwrap(), Some(ReasoningEffort::Default));
        assert_eq!(ReasoningEffort::parse("disabled").unwrap(), Some(ReasoningEffort::Disabled));
        assert_eq!(ReasoningEffort::parse("high").unwrap(), Some(ReasoningEffort::High));
        assert_eq!(ReasoningEffort::parse("low").unwrap(), Some(ReasoningEffort::Low));
    }

    #[test]
    fn parse_rejects_unknown() {
        assert!(ReasoningEffort::parse("extreme").is_err());
    }

    #[test]
    fn config_str_values() {
        assert_eq!(ReasoningEffort::config_str(None), "default");
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
    fn cycle_within_back_three_levels() {
        use ReasoningEffort::*;
        let levels = &[Low, Medium, High];
        assert_eq!(ReasoningEffort::cycle_within_back(None, levels), Some(High));
        assert_eq!(ReasoningEffort::cycle_within_back(Some(High), levels), Some(Medium));
        assert_eq!(ReasoningEffort::cycle_within_back(Some(Medium), levels), Some(Low));
        assert_eq!(ReasoningEffort::cycle_within_back(Some(Low), levels), None);
    }

    #[test]
    fn cycle_within_back_empty_returns_none() {
        assert_eq!(ReasoningEffort::cycle_within_back(None, &[]), None);
        assert_eq!(ReasoningEffort::cycle_within_back(Some(ReasoningEffort::Low), &[]), None);
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
