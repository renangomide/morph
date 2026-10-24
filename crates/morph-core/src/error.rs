// SPDX-License-Identifier: GPL-3.0-or-later
//! Task errors: one short plain-English sentence for the user, plus technical details.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// The user cancelled. Not shown as a failure.
    Cancelled,
    /// Input can't be read or decoded.
    BadInput,
    /// The output folder can't be written.
    OutputNotWritable,
    /// Format/option combination not supported.
    Unsupported,
    /// Needs Microsoft Office or LibreOffice and neither is installed
    /// (the window shows the "install LibreOffice" banner).
    MissingEngine,
    /// Anything else (engine failure, I/O, bug).
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct ConvertError {
    pub kind: ErrorKind,
    /// Shown in the results row, e.g. "This file seems to be damaged."
    pub message: String,
    /// Shown under "Details" (technical error, engine stderr tail).
    pub details: String,
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.details.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{} ({})", self.message, self.details)
        }
    }
}

impl ConvertError {
    pub fn new(kind: ErrorKind, message: impl Into<String>, details: impl ToString) -> Self {
        ConvertError {
            kind,
            message: message.into(),
            details: details.to_string(),
        }
    }

    pub fn cancelled() -> Self {
        ConvertError::new(ErrorKind::Cancelled, "Cancelled.", "")
    }

    pub fn damaged(details: impl ToString) -> Self {
        ConvertError::new(
            ErrorKind::BadInput,
            "This file seems to be damaged or isn't a supported image.",
            details,
        )
    }

    pub fn unsupported(message: impl Into<String>, details: impl ToString) -> Self {
        ConvertError::new(ErrorKind::Unsupported, message, details)
    }

    pub fn not_writable(details: impl ToString) -> Self {
        ConvertError::new(
            ErrorKind::OutputNotWritable,
            "Can't save in this folder. Use \"Choose folder…\" to pick another one.",
            details,
        )
    }

    pub fn other(message: impl Into<String>, details: impl ToString) -> Self {
        ConvertError::new(ErrorKind::Other, message, details)
    }

    /// A conversion whose code isn't written yet (stubs during a milestone).
    pub fn not_implemented(what: &str) -> Self {
        ConvertError::unsupported(
            "This conversion isn't available yet.",
            format!("not implemented yet: {what}"),
        )
    }

    pub fn is_cancelled(&self) -> bool {
        self.kind == ErrorKind::Cancelled
    }
}

impl From<std::io::Error> for ConvertError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem => {
                ConvertError::not_writable(e)
            }
            std::io::ErrorKind::NotFound => {
                ConvertError::new(ErrorKind::BadInput, "The file was moved or deleted.", e)
            }
            _ => ConvertError::other(
                "A file operation failed. Check that the file and its folder are still there, then try again.",
                e,
            ),
        }
    }
}
