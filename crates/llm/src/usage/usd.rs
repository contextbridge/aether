use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, AddAssign};

/// A monetary amount denominated in US dollars.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
#[schemars(transparent)]
pub struct Usd(f64);

impl Usd {
    pub const ZERO: Self = Self(0.0);

    pub const fn new(value: f64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

impl Add for Usd {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl AddAssign for Usd {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl fmt::Display for Usd {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
