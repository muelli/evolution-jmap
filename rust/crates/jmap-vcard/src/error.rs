// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Describe vCard parse failures.

/// Explain why a string cannot be read as a vCard.
/// The parser treats unknown semantic fields as absent. Partial contacts still load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VCardError {
    /// The input does not start with `BEGIN:VCARD`.
    NotAVCard,
    /// `BEGIN:VCARD` was never closed by `END:VCARD`.
    Unterminated,
    /// A content line has no `name:value` structure.
    Malformed(String),
}

impl std::fmt::Display for VCardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAVCard => f.write_str("not a vCard: missing BEGIN:VCARD"),
            Self::Unterminated => f.write_str("truncated vCard: missing END:VCARD"),
            Self::Malformed(line) => write!(f, "malformed vCard content line: {line}"),
        }
    }
}

impl std::error::Error for VCardError {}
