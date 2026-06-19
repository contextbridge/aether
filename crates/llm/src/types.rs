use std::fmt::Display;

use chrono::{DateTime, TimeZone};
use serde::{Deserialize, Serialize};

/// A newtype wrapper for ISO 8601 timestamp strings
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IsoString(pub String);

impl IsoString {
    /// Create a new `IsoString` from the current time
    pub fn now() -> Self {
        Self(chrono::Utc::now().to_rfc3339())
    }

    /// Create an `IsoString` from a chrono `DateTime`
    pub fn from_datetime<T: TimeZone>(datetime: &DateTime<T>) -> Self
    where
        T::Offset: Display,
    {
        Self(datetime.to_rfc3339())
    }

    /// Get the inner string value
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
