use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, AddAssign};

/// A count of tokens. Sums saturate instead of wrapping.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
#[schemars(transparent)]
pub struct Tokens(u64);

impl Tokens {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    pub const fn saturating_add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }

    pub const fn saturating_sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}

impl Add for Tokens {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        self.saturating_add(rhs)
    }
}

impl AddAssign for Tokens {
    fn add_assign(&mut self, rhs: Self) {
        *self = self.saturating_add(rhs);
    }
}

impl From<u32> for Tokens {
    fn from(value: u32) -> Self {
        Self(value.into())
    }
}

impl From<Tokens> for u64 {
    fn from(value: Tokens) -> Self {
        value.0
    }
}

impl TryFrom<Tokens> for i64 {
    type Error = std::num::TryFromIntError;
    fn try_from(value: Tokens) -> Result<Self, Self::Error> {
        value.0.try_into()
    }
}

impl From<Tokens> for f64 {
    #[allow(clippy::cast_precision_loss)]
    fn from(value: Tokens) -> Self {
        value.0 as f64
    }
}

impl fmt::Display for Tokens {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Token counts for a single LLM call or an aggregate of calls. Providers fill
/// in only the dimensions they report; the rest stay `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenUsage {
    pub input_tokens: Tokens,
    pub output_tokens: Tokens,
    #[serde(default)]
    pub cache_read_tokens: Option<Tokens>,
    #[serde(default)]
    pub cache_creation_tokens: Option<Tokens>,
    #[serde(default)]
    pub input_audio_tokens: Option<Tokens>,
    #[serde(default)]
    pub input_video_tokens: Option<Tokens>,
    #[serde(default)]
    pub reasoning_tokens: Option<Tokens>,
    #[serde(default)]
    pub output_audio_tokens: Option<Tokens>,
    #[serde(default)]
    pub accepted_prediction_tokens: Option<Tokens>,
    #[serde(default)]
    pub rejected_prediction_tokens: Option<Tokens>,
}

impl TokenUsage {
    pub fn new(input_tokens: u64, output_tokens: u64) -> Self {
        Self { input_tokens: Tokens::new(input_tokens), output_tokens: Tokens::new(output_tokens), ..Self::default() }
    }

    pub fn is_zero(self) -> bool {
        let reported = [
            self.cache_read_tokens,
            self.cache_creation_tokens,
            self.input_audio_tokens,
            self.input_video_tokens,
            self.reasoning_tokens,
            self.output_audio_tokens,
            self.accepted_prediction_tokens,
            self.rejected_prediction_tokens,
        ];
        self.total_tokens().is_zero() && reported.iter().all(|dimension| dimension.unwrap_or_default().is_zero())
    }

    pub fn total_tokens(self) -> Tokens {
        self.input_tokens + self.output_tokens
    }
}

impl Add for TokenUsage {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self {
            input_tokens: self.input_tokens + rhs.input_tokens,
            output_tokens: self.output_tokens + rhs.output_tokens,
            cache_read_tokens: add_reported(self.cache_read_tokens, rhs.cache_read_tokens),
            cache_creation_tokens: add_reported(self.cache_creation_tokens, rhs.cache_creation_tokens),
            input_audio_tokens: add_reported(self.input_audio_tokens, rhs.input_audio_tokens),
            input_video_tokens: add_reported(self.input_video_tokens, rhs.input_video_tokens),
            reasoning_tokens: add_reported(self.reasoning_tokens, rhs.reasoning_tokens),
            output_audio_tokens: add_reported(self.output_audio_tokens, rhs.output_audio_tokens),
            accepted_prediction_tokens: add_reported(self.accepted_prediction_tokens, rhs.accepted_prediction_tokens),
            rejected_prediction_tokens: add_reported(self.rejected_prediction_tokens, rhs.rejected_prediction_tokens),
        }
    }
}

impl AddAssign for TokenUsage {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

fn add_reported(lhs: Option<Tokens>, rhs: Option<Tokens>) -> Option<Tokens> {
    match (lhs, rhs) {
        (None, None) => None,
        _ => Some(lhs.unwrap_or_default() + rhs.unwrap_or_default()),
    }
}
