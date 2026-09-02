// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The pure half of the page: widget values as data, mapped to and from the
//! RFC 8621 §8 `VacationResponse` object.

use std::ffi::CStr;

use jmap_backend_core::i18n::N_;
use jmap_proto::UtcDate;
use jmap_proto::mail::VacationResponse;
use serde_json::{Value, json};

/// What the widgets hold: dates as the entry text (`YYYY-MM-DD` or empty),
/// subject and body as typed. Comparing a snapshot against the loaded
/// baseline is the dirty check — no signal bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VacationForm {
    pub enabled: bool,
    pub from_date: String,
    pub to_date: String,
    pub subject: String,
    pub body: String,
}

/// The message behind a refused date entry.
pub const BAD_DATE: &CStr = N_(c"Dates take the form YYYY-MM-DD; leave one empty for no limit");

impl VacationForm {
    /// The server's object as widget values. A missing date is an empty
    /// entry; a missing subject or body is empty text (the server generates
    /// its defaults for those, RFC 8621 §8).
    pub fn from_response(response: &VacationResponse) -> Self {
        Self {
            enabled: response.is_enabled,
            from_date: date_prefix(response.from_date.as_ref()),
            to_date: date_prefix(response.to_date.as_ref()),
            subject: response.subject.clone().unwrap_or_default(),
            body: response.text_body.clone().unwrap_or_default(),
        }
    }

    /// The `VacationResponse/set` update patch this form asks for, every
    /// field written: the page edits the whole object, so what the widgets
    /// show is what the server should hold. Empty subject/body are `null` —
    /// "let the server pick" — rather than empty strings.
    pub fn patch(&self) -> Result<Value, &'static CStr> {
        Ok(json!({
            "isEnabled": self.enabled,
            "fromDate": date_value(&self.from_date)?,
            "toDate": date_value(&self.to_date)?,
            "subject": nullable(&self.subject),
            "textBody": nullable(&self.body),
        }))
    }
}

/// `2026-09-01T00:00:00Z` → `2026-09-01`; absent → empty.
fn date_prefix(date: Option<&UtcDate>) -> String {
    date.map(|date| {
        date.as_str()
            .split('T')
            .next()
            .unwrap_or_default()
            .to_owned()
    })
    .unwrap_or_default()
}

/// An entry's text as the patch value: empty means `null` (no limit), and
/// anything else has to look like a date before a midnight time is appended.
fn date_value(entry: &str) -> Result<Value, &'static CStr> {
    let entry = entry.trim();
    if entry.is_empty() {
        return Ok(Value::Null);
    }
    let shaped = entry.len() == 10
        && entry.char_indices().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        });
    if !shaped {
        return Err(BAD_DATE);
    }
    Ok(Value::String(format!("{entry}T00:00:00Z")))
}

fn nullable(text: &str) -> Value {
    let text = text.trim();
    if text.is_empty() {
        Value::Null
    } else {
        Value::String(text.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_response_becomes_widget_values_and_back() {
        let response: VacationResponse = serde_json::from_value(json!({
            "id": "singleton",
            "isEnabled": true,
            "fromDate": "2026-09-10T00:00:00Z",
            "toDate": null,
            "subject": "Away",
            "textBody": "Back on the 20th.",
        }))
        .unwrap();

        let form = VacationForm::from_response(&response);
        assert_eq!(form.from_date, "2026-09-10");
        assert_eq!(form.to_date, "");
        assert!(form.enabled);

        let patch = form.patch().unwrap();
        assert_eq!(patch["isEnabled"], true);
        assert_eq!(patch["fromDate"], "2026-09-10T00:00:00Z");
        assert_eq!(patch["toDate"], Value::Null);
        assert_eq!(patch["subject"], "Away");
        assert_eq!(patch["textBody"], "Back on the 20th.");
    }

    #[test]
    fn empty_text_asks_for_the_server_default() {
        let form = VacationForm {
            enabled: false,
            subject: "  ".to_owned(),
            ..VacationForm::default()
        };
        let patch = form.patch().unwrap();
        assert_eq!(patch["subject"], Value::Null);
        assert_eq!(patch["textBody"], Value::Null);
    }

    #[test]
    fn a_malformed_date_is_refused() {
        for bad in ["tomorrow", "2026-9-1", "2026/09/01", "2026-09-01x"] {
            let form = VacationForm {
                from_date: bad.to_owned(),
                ..VacationForm::default()
            };
            assert_eq!(form.patch(), Err(BAD_DATE), "{bad} must be refused");
        }
    }
}
