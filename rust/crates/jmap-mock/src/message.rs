// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Read a minimal RFC 5322 subset for `Email/import`.
//!
//! Most modules serialize structured `Email` values to message bytes.
//! `Email/import` reverses that path. This module parses a small raw-byte
//! subset.
//!
//! Scope: unfolded headers, address lists, message IDs, and preview text.
//! This module skips MIME trees, RFC 2047 encoded words, temporal headers,
//! and address groups. Tests that need those features seed messages directly.

use jmap_proto::mail::EmailAddress;

/// Preview character limit. Keep this value aligned with seeded messages.
const PREVIEW: usize = 64;

/// Parsed message fields that this server uses.
pub(crate) struct Message {
    /// Header fields in wire order. Use lowercase names and unfolded values.
    fields: Vec<(String, String)>,
    /// Message body after the header block, interpreted as UTF-8 text.
    body: String,
}

impl Message {
    /// Parse `source`. Return `None` for invalid UTF-8 or header syntax.
    pub(crate) fn read(source: &[u8]) -> Option<Self> {
        let source = std::str::from_utf8(source).ok()?;
        let (headers, body) = split(source);

        let mut fields: Vec<(String, String)> = Vec::new();
        for line in headers.split("\r\n").flat_map(|line| line.split('\n')) {
            if line.is_empty() {
                continue;
            }
            // Treat a leading space or tab as a folded continuation line.
            if line.starts_with([' ', '\t']) {
                let (_, value) = fields.last_mut()?;
                value.push(' ');
                value.push_str(line.trim());
                continue;
            }
            let (name, value) = line.split_once(':')?;
            if name.is_empty() || !name.bytes().all(is_field_name) {
                return None;
            }
            fields.push((name.to_ascii_lowercase(), value.trim().to_owned()));
        }
        if fields.is_empty() {
            return None;
        }

        Some(Self {
            fields,
            body: body.to_owned(),
        })
    }

    /// Return the first value for a lowercase header name.
    fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.as_str())
            .filter(|value| !value.is_empty())
    }

    pub(crate) fn subject(&self) -> Option<String> {
        self.field("subject").map(str::to_owned)
    }

    /// Parse one address-list header into JMAP addresses.
    pub(crate) fn addresses(&self, name: &str) -> Option<Vec<EmailAddress>> {
        let addresses: Vec<EmailAddress> = split_addresses(self.field(name)?)
            .into_iter()
            .filter_map(address)
            .collect();
        (!addresses.is_empty()).then_some(addresses)
    }

    /// Parse message-id style headers. Strip optional angle brackets.
    pub(crate) fn message_ids(&self, name: &str) -> Option<Vec<String>> {
        let ids: Vec<String> = self
            .field(name)?
            .split_whitespace()
            .flat_map(|token| token.split(','))
            .filter_map(|token| {
                let id = token.trim().trim_start_matches('<').trim_end_matches('>');
                (!id.is_empty()).then(|| id.to_owned())
            })
            .collect();
        (!ids.is_empty()).then_some(ids)
    }

    /// Build a one-line preview from the body start.
    pub(crate) fn preview(&self) -> Option<String> {
        let preview: String = self
            .body
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(PREVIEW)
            .collect();
        (!preview.is_empty()).then_some(preview)
    }
}

/// Split the message into headers and body at the first blank line.
fn split(source: &str) -> (&str, &str) {
    match source.find("\r\n\r\n") {
        Some(end) => (&source[..end], &source[end + 4..]),
        None => match source.find("\n\n") {
            Some(end) => (&source[..end], &source[end + 2..]),
            None => (source, ""),
        },
    }
}

/// Return true if `byte` is valid RFC 5322 `ftext` for a field name.
fn is_field_name(byte: u8) -> bool {
    (33..=57).contains(&byte) || (59..=126).contains(&byte)
}

/// Split an address list on top-level commas.
fn split_addresses(list: &str) -> Vec<&str> {
    let mut addresses = Vec::new();
    let mut quoted = false;
    let mut angled = false;
    let mut start = 0;
    for (index, character) in list.char_indices() {
        match character {
            '"' => quoted = !quoted,
            '<' if !quoted => angled = true,
            '>' if !quoted => angled = false,
            ',' if !quoted && !angled => {
                addresses.push(&list[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    addresses.push(&list[start..]);
    addresses
}

/// Parse one address in `Name <local@domain>` or bare `local@domain` form.
fn address(address: &str) -> Option<EmailAddress> {
    let address = address.trim();
    if address.is_empty() {
        return None;
    }
    let Some((name, rest)) = address.split_once('<') else {
        return Some(EmailAddress::from_email(address));
    };
    let email = rest.trim_end().trim_end_matches('>').trim();
    if email.is_empty() {
        return None;
    }
    let name = name.trim().trim_matches('"').trim();
    let mut result = EmailAddress::from_email(email);
    if !name.is_empty() {
        result = result.with_name(name);
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folded_header_is_one_value() {
        let message = Message::read(b"Subject: one\r\n two\r\n\tthree\r\n\r\nbody\r\n").unwrap();
        assert_eq!(message.subject().as_deref(), Some("one two three"));
    }

    #[test]
    fn a_message_may_have_no_body() {
        let message = Message::read(b"Subject: nothing follows\r\n").unwrap();
        assert_eq!(message.subject().as_deref(), Some("nothing follows"));
        assert_eq!(message.preview(), None);
    }

    #[test]
    fn bare_line_feeds_are_read_too() {
        let message = Message::read(b"Subject: unix\nFrom: a@b\n\nbody").unwrap();
        assert_eq!(message.subject().as_deref(), Some("unix"));
        assert_eq!(message.preview().as_deref(), Some("body"));
    }

    #[test]
    fn what_is_not_a_message_is_refused() {
        // Reject missing fields, invalid header syntax, and non-UTF-8 bytes.
        assert!(Message::read(b"").is_none());
        assert!(Message::read(b"this was never a message").is_none());
        assert!(Message::read(b"Subject: fine\r\nnot a field\r\n").is_none());
        assert!(Message::read(b" continues nothing\r\nSubject: fine\r\n").is_none());
        assert!(Message::read(b": empty name\r\n").is_none());
        assert!(Message::read(&[0xff, 0xfe, b':', b' ', b'x']).is_none());
    }

    #[test]
    fn an_empty_header_reads_as_absent() {
        let message = Message::read(b"Subject:\r\nTo:  \r\n\r\nbody").unwrap();
        assert_eq!(message.subject(), None);
        assert_eq!(message.addresses("to"), None);
    }

    #[test]
    fn the_first_of_a_repeated_header_wins() {
        let message = Message::read(b"Subject: first\r\nSubject: second\r\n").unwrap();
        assert_eq!(message.subject().as_deref(), Some("first"));
    }

    #[test]
    fn a_display_name_may_hold_a_comma() {
        let message =
            Message::read(b"To: \"Doe, John\" <j@example.com>, plain@example.com\r\n").unwrap();
        let to = message.addresses("to").unwrap();
        assert_eq!(to.len(), 2);
        assert_eq!(to[0].name.as_deref(), Some("Doe, John"));
        assert_eq!(to[0].email, "j@example.com");
        assert_eq!(to[1].name, None);
        assert_eq!(to[1].email, "plain@example.com");
    }

    #[test]
    fn message_ids_lose_their_brackets() {
        let message =
            Message::read(b"References: <one@example.com> <two@example.com>\r\n").unwrap();
        assert_eq!(
            message.message_ids("references").unwrap(),
            vec!["one@example.com".to_owned(), "two@example.com".to_owned()]
        );
    }

    #[test]
    fn a_preview_is_one_line_of_the_body() {
        let message =
            Message::read(b"Subject: s\r\n\r\nfirst line\r\n\r\nsecond   line\r\n").unwrap();
        assert_eq!(message.preview().as_deref(), Some("first line second line"));
    }

    #[test]
    fn a_preview_stops_at_sixty_four_characters() {
        let body = "x".repeat(100);
        let message = Message::read(format!("Subject: s\r\n\r\n{body}").as_bytes()).unwrap();
        assert_eq!(message.preview().unwrap().chars().count(), PREVIEW);
    }
}
