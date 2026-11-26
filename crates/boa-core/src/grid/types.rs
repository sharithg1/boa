//! The grid logic's error type. The catalog and result shapes it works with
//! (`Value`, `ResultColumn`, `Table`, …) are the backend's, in
//! [`crate::model`].

use std::fmt;
/// An error of the grid logic (a table that can't be edited, a copy format
/// that doesn't apply), with a message for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridError {
    pub message: String,
}

impl GridError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for GridError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GridError {}
