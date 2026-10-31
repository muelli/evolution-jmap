// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Hostile server scenario definitions and CLI parsing.

use std::fmt;
use std::str::FromStr;

/// Hostile server scenario to simulate an adversarial or broken server.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum HostileScenario {
    OversizedResponse,
    DeepJson,
    NumbersOutOfRange,
    DangerousIds,
    BlobSizeMismatch,
    SessionUrlsOtherOrigin,
    SessionUrlsFile,
    RedirectOtherOrigin,
    WrongMethodCallId,
    DuplicateMethodCallId,
    OutOfOrderResponses,
    ReplayedResponse,
    StateBackwards,
    EventsourceUnknown,
}

impl HostileScenario {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OversizedResponse => "oversized-response",
            Self::DeepJson => "deep-json",
            Self::NumbersOutOfRange => "numbers-out-of-range",
            Self::DangerousIds => "dangerous-ids",
            Self::BlobSizeMismatch => "blob-size-mismatch",
            Self::SessionUrlsOtherOrigin => "session-urls-other-origin",
            Self::SessionUrlsFile => "session-urls-file",
            Self::RedirectOtherOrigin => "redirect-other-origin",
            Self::WrongMethodCallId => "wrong-method-call-id",
            Self::DuplicateMethodCallId => "duplicate-method-call-id",
            Self::OutOfOrderResponses => "out-of-order-responses",
            Self::ReplayedResponse => "replayed-response",
            Self::StateBackwards => "state-backwards",
            Self::EventsourceUnknown => "eventsource-unknown",
        }
    }
}

impl fmt::Display for HostileScenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseHostileScenarioError(pub String);

impl fmt::Display for ParseHostileScenarioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown hostile scenario: {}", self.0)
    }
}

impl std::error::Error for ParseHostileScenarioError {}

impl FromStr for HostileScenario {
    type Err = ParseHostileScenarioError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.trim().to_lowercase().replace('_', "-");
        match normalized.as_str() {
            "oversized-response" | "oversized" => Ok(Self::OversizedResponse),
            "deep-json" | "json-depth" | "json-nested-deep" | "json-10000-deep" => {
                Ok(Self::DeepJson)
            }
            "numbers-out-of-range" | "huge-numbers" | "numbers-beyond-2-53" => {
                Ok(Self::NumbersOutOfRange)
            }
            "dangerous-ids" | "malformed-ids" | "special-ids" => Ok(Self::DangerousIds),
            "blob-size-mismatch" | "mismatched-blob-size" => Ok(Self::BlobSizeMismatch),
            "session-urls-other-origin" | "session-urls-foreign" | "session-foreign-origin" => {
                Ok(Self::SessionUrlsOtherOrigin)
            }
            "session-urls-file" | "session-file-urls" => Ok(Self::SessionUrlsFile),
            "redirect-other-origin" | "redirect-foreign-origin" => Ok(Self::RedirectOtherOrigin),
            "wrong-method-call-id" | "wrong-call-id" => Ok(Self::WrongMethodCallId),
            "duplicate-method-call-id" | "duplicate-call-id" => Ok(Self::DuplicateMethodCallId),
            "out-of-order-responses" | "out-of-order" | "out-of-order-methods" => {
                Ok(Self::OutOfOrderResponses)
            }
            "replayed-response" | "replay-response" | "replayed" => Ok(Self::ReplayedResponse),
            "state-backwards" | "state-regression" | "state-going-backwards" => {
                Ok(Self::StateBackwards)
            }
            "eventsource-unknown"
            | "eventsource-unknown-types"
            | "eventsource-unknown-accounts" => Ok(Self::EventsourceUnknown),
            _ => Err(ParseHostileScenarioError(s.to_string())),
        }
    }
}
