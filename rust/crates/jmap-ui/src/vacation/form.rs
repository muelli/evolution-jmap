// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The pure part of the page. It maps widget values to and from the
//! RFC 8621 §8 `VacationResponse` object.

use std::ffi::CStr;

use jmap_backend_core::i18n::N_;
use jmap_proto::UtcDate;
use jmap_proto::mail::VacationResponse;
use serde_json::{Value, json};

/// The widget values. A date is `YYYY-MM-DD`, or empty when the date field
/// has no date. The subject and the body are the text that the user typed.
/// The page finds changes when it compares a snapshot with the loaded
/// baseline, so it does not record signals.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VacationForm {
    pub enabled: bool,
    pub from_date: String,
    pub to_date: String,
    pub subject: String,
    pub body: String,
}

/// The message for a date that is not valid.
pub const BAD_DATE: &CStr = N_(c"Dates take the form YYYY-MM-DD; leave one empty for no limit");

/// The message for a last day that is before the first day.
pub const BACKWARDS: &CStr =
    N_(c"The last day is before the first day — swap them, or clear one to leave it open");

impl VacationForm {
    /// Converts the server object to widget values. A missing date becomes an
    /// empty date. A missing subject or body becomes empty text (the server
    /// supplies default values for them, RFC 8621 §8).
    pub fn from_response(response: &VacationResponse) -> Self {
        Self {
            enabled: response.is_enabled,
            from_date: date_prefix(response.from_date.as_ref()),
            to_date: date_prefix(response.to_date.as_ref()),
            subject: response.subject.clone().unwrap_or_default(),
            body: response.text_body.clone().unwrap_or_default(),
        }
    }

    /// The `VacationResponse/set` update patch for this form. The patch
    /// writes all fields, because the page edits the full object. An empty
    /// subject or body becomes `null` (the server selects the value), not an
    /// empty string.
    pub fn patch(&self) -> Result<Value, &'static CStr> {
        let from = date_value(&self.from_date)?;
        let to = date_value(&self.to_date)?;
        // A range that ends before it starts is an error. The server accepts
        // it, but then sends no replies. Both bounds are `YYYY-MM-DDT…Z`, so
        // a string comparison is also a time comparison.
        if let (Value::String(from), Value::String(to)) = (&from, &to)
            && to < from
        {
            return Err(BACKWARDS);
        }
        Ok(json!({
            "isEnabled": self.enabled,
            "fromDate": from,
            "toDate": to,
            "subject": nullable(&self.subject),
            "textBody": nullable(&self.body),
        }))
    }
}

/// Changes `2026-09-01T00:00:00Z` to `2026-09-01`, and no date to empty.
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

/// Converts a date value to the patch value. Empty becomes `null` (no
/// limit). Other text must be a calendar date `YYYY-MM-DD`, then it gets
/// the time midnight UTC.
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
    if !shaped || !is_calendar_date(entry) {
        return Err(BAD_DATE);
    }
    Ok(Value::String(format!("{entry}T00:00:00Z")))
}

/// Whether a `YYYY-MM-DD` text names a day that the Gregorian calendar has.
fn is_calendar_date(entry: &str) -> bool {
    let number = |range: std::ops::Range<usize>| entry[range].parse::<u32>().ok();
    let (Some(year), Some(month), Some(day)) = (number(0..4), number(5..7), number(8..10)) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let last = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=last).contains(&day)
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

    /// The form refuses a range that ends before it starts. It accepts a
    /// range where the first day and the last day are equal (an absence of
    /// one day).
    #[test]
    fn a_backwards_range_is_refused_but_a_single_day_is_not() {
        let backwards = VacationForm {
            enabled: true,
            from_date: "2026-09-10".to_owned(),
            to_date: "2026-09-01".to_owned(),
            ..VacationForm::default()
        };
        assert_eq!(backwards.patch(), Err(BACKWARDS));

        let same_day = VacationForm {
            to_date: "2026-09-10".to_owned(),
            ..backwards.clone()
        };
        assert!(same_day.patch().is_ok(), "one day off must be allowed");

        // A range with one empty bound is always in the correct order.
        for (from, to) in [("2026-09-10", ""), ("", "2026-09-01")] {
            let open = VacationForm {
                from_date: from.to_owned(),
                to_date: to.to_owned(),
                ..VacationForm::default()
            };
            assert!(open.patch().is_ok(), "{from:?}..{to:?} must be allowed");
        }
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

    #[test]
    fn a_date_that_no_calendar_has_is_refused() {
        for bad in [
            "2026-02-31",
            "2026-13-45",
            "2026-00-10",
            "2026-04-00",
            "2025-02-29",
        ] {
            let form = VacationForm {
                to_date: bad.to_owned(),
                ..VacationForm::default()
            };
            assert_eq!(form.patch(), Err(BAD_DATE), "{bad} must be refused");
        }
    }

    #[test]
    fn the_leap_day_of_a_leap_year_is_a_date() {
        for good in ["2028-02-29", "2000-02-29", "2026-12-31"] {
            let form = VacationForm {
                from_date: good.to_owned(),
                ..VacationForm::default()
            };
            assert!(form.patch().is_ok(), "{good} must be accepted");
        }
    }
}
