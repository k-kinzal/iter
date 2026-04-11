//! [`Prompt`] — a rendered prompt ready to hand to an
//! [`Agent`](crate::agent::Agent).

use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Serialize};

/// A rendered prompt that is ready to hand to an [`Agent`](crate::agent::Agent).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Prompt(String);

impl Prompt {
    /// Wrap a string as a [`Prompt`].
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// Borrow the rendered text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the prompt and return the underlying [`String`].
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl Deref for Prompt {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for Prompt {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for Prompt {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Prompt {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Display for Prompt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
