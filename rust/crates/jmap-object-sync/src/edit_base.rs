// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The base of an edit: the server version that the user started the edit from.
//!
//! An editor can stay open while a refresh replaces the cached object and its
//! extra. Thus the extra at save time is not always the base of the edit. Each
//! object that goes to EDS carries a stamp, the [`revision_of`] its server
//! version. The extra keeps the most recent server versions. A save uses the
//! version that the stamp names as the base.

pub use crate::revision_of;

use crate::OutOfSync;

/// The property that carries the stamp in the iCalendar or vCard text.
pub const STAMP: &str = "X-JMAP-BASE";

/// The number of server versions that an extra keeps.
pub const KEPT: usize = 4;

/// Adds the stamp `revision` to each `kind` component (`VEVENT`, `VCARD`) of `text`.
pub fn stamp(text: &str, kind: &str, revision: &str) -> String {
    let end = format!("END:{kind}");
    let mut stamped = String::with_capacity(text.len() + 32);
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        if content.eq_ignore_ascii_case(&end) {
            stamped.push_str(STAMP);
            stamped.push(':');
            stamped.push_str(revision);
            stamped.push_str(&line[content.len()..]);
            if line.len() == content.len() {
                stamped.push_str("\r\n");
            }
        }
        stamped.push_str(line);
    }
    stamped
}

/// Removes each stamp line from `text`. Also returns the first stamp.
pub fn unstamp(text: &str) -> (String, Option<String>) {
    let mut kept = String::with_capacity(text.len());
    let mut found = None;
    let mut in_stamp = false;
    for line in text.split_inclusive('\n') {
        // A folded line continues the line before it (RFC 5545 section 3.1).
        if in_stamp && line.starts_with([' ', '\t']) {
            continue;
        }
        in_stamp = is_stamp(line);
        if in_stamp {
            if found.is_none() {
                found = line
                    .split_once(':')
                    .map(|(_, value)| value.trim().to_owned());
            }
            continue;
        }
        kept.push_str(line);
    }
    (kept, found)
}

fn is_stamp(line: &str) -> bool {
    line.get(..STAMP.len())
        .is_some_and(|name| name.eq_ignore_ascii_case(STAMP))
        && matches!(line.as_bytes().get(STAMP.len()), Some(b':' | b';'))
}

/// The extra for the server version `current`, after the extra `previous`.
///
/// The extra keeps `current` first, then the older versions of `previous`, at
/// most [`KEPT`] in total. One version is the plain text, as before the
/// stamps. More than one is a JSON array of texts.
pub fn extra(current: &str, previous: Option<&str>) -> String {
    let mut versions = vec![current.to_owned()];
    for version in previous.map(versions_of).unwrap_or_default() {
        if versions.len() == KEPT {
            break;
        }
        if !versions.contains(&version) {
            versions.push(version);
        }
    }
    if versions.len() == 1 {
        versions.swap_remove(0)
    } else {
        serde_json::Value::from(versions).to_string()
    }
}

/// The server versions in `extra`, the most recent first.
pub fn versions_of(extra: &str) -> Vec<String> {
    if extra.starts_with('[') {
        serde_json::from_str(extra).unwrap_or_default()
    } else if extra.is_empty() {
        Vec::new()
    } else {
        vec![extra.to_owned()]
    }
}

/// The base of an edit with the stamp `stamp`, from the cached `extra`.
///
/// An edit without a stamp started before the stamps, so its base is the most
/// recent version. A stamp that names no kept version is [`OutOfSync`]: any
/// base in its place can send old values back as the user's edit.
pub fn base_for(extra: Option<&str>, stamp: Option<&str>) -> Result<Option<String>, OutOfSync> {
    let Some(extra) = extra else {
        return Ok(None);
    };
    let mut versions = versions_of(extra).into_iter();
    match stamp {
        None => Ok(versions.next()),
        Some(stamp) => versions
            .find(|version| revision_of(version) == stamp)
            .map(Some)
            .ok_or(OutOfSync),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1: &str =
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:a\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    const V2: &str =
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:b\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn stamp_adds_one_line_before_each_end_of_the_kind() {
        assert_eq!(
            stamp(V1, "VEVENT", "0123"),
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:a\r\nX-JMAP-BASE:0123\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        assert_eq!(
            stamp("BEGIN:VCARD\nFN:a\nend:vcard", "VCARD", "9"),
            "BEGIN:VCARD\nFN:a\nX-JMAP-BASE:9\r\nend:vcard"
        );
    }

    #[test]
    fn unstamp_gives_back_the_text_and_the_first_stamp() {
        let stamped = stamp(V1, "VEVENT", "0123");
        assert_eq!(unstamp(&stamped), (V1.to_owned(), Some("0123".to_owned())));
        assert_eq!(unstamp(V1), (V1.to_owned(), None));
    }

    #[test]
    fn unstamp_removes_a_folded_stamp_and_a_stamp_with_parameters() {
        let text = "BEGIN:VCARD\r\nx-jmap-base;X=1:ab\r\n cd\r\nX-JMAP-BASE:ef\r\nX-JMAP-BASEMENT:1\r\nEND:VCARD\r\n";
        assert_eq!(
            unstamp(text),
            (
                "BEGIN:VCARD\r\nX-JMAP-BASEMENT:1\r\nEND:VCARD\r\n".to_owned(),
                Some("ab".to_owned())
            )
        );
    }

    #[test]
    fn extra_keeps_the_recent_versions_first_without_duplicates() {
        assert_eq!(extra(V1, None), V1);
        assert_eq!(extra(V1, Some(V1)), V1);
        let two = extra(V2, Some(V1));
        assert_eq!(versions_of(&two), [V2, V1]);
        assert_eq!(versions_of(&extra(V1, Some(&two))), [V1, V2]);
        let mut all = String::new();
        for index in 0..10 {
            all = extra(&format!("v{index}"), Some(&all));
        }
        assert_eq!(versions_of(&all), ["v9", "v8", "v7", "v6"]);
    }

    #[test]
    fn base_for_picks_the_stamped_version() {
        let two = extra(V2, Some(V1));
        assert_eq!(
            base_for(Some(&two), Some(&revision_of(V1))),
            Ok(Some(V1.to_owned()))
        );
        assert_eq!(
            base_for(Some(&two), Some(&revision_of(V2))),
            Ok(Some(V2.to_owned()))
        );
        assert_eq!(base_for(Some(&two), None), Ok(Some(V2.to_owned())));
        assert_eq!(base_for(Some(V1), None), Ok(Some(V1.to_owned())));
        assert_eq!(base_for(None, Some("0123")), Ok(None));
    }

    #[test]
    fn base_for_refuses_a_stamp_that_names_no_kept_version() {
        assert_eq!(base_for(Some(V2), Some(&revision_of(V1))), Err(OutOfSync));
    }
}
