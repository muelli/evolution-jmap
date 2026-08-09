// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading an RFC 5322 message back out of bytes, for `Email/import`.
//!
//! Everywhere else this server works the other way round: it holds an `Email`
//! and [`crate::mail`] writes a message out of it, which is backwards for a mail
//! server and right for a test fixture. An import is the one method where the
//! bytes come first — RFC 8621 §4.8 takes a blob and has the server derive the
//! `Email` — so this is the only parser in the crate.
//!
//! ## How little of RFC 5322 is here, and why that is a choice
//!
//! Enough to fill the properties a message list shows and no more: the header
//! block, unfolded; the body, for a preview; address lists; message ids. What is
//! deliberately absent:
//!
//! - **The MIME tree.** No `bodyStructure`, `textBody` or `bodyValues` comes out
//!   of an import. A seeded message has them because the seed *is* the structure;
//!   deriving them from bytes means a MIME parser, and a half-written one would
//!   make this server a worse thing to test against than one that visibly has
//!   none. What a client downloads is the raw blob, unchanged, which is how a
//!   message actually gets read.
//! - **Encoded words (RFC 2047).** A `Subject` arrives as the octets it was sent
//!   as, exactly as [`crate::mail`]'s writer emits display names without
//!   encoding them.
//! - **Dates.** No `Received` or `Date` header is turned into a `UtcDate`: that
//!   is calendar arithmetic across a zone offset, and this crate does none (see
//!   `jmap_proto::UtcDate`). An import therefore takes the `receivedAt` it is
//!   given, or the mock's fixed clock — see [`crate::mail::email_import`].
//! - **Address groups** (`managers: a@b, c@d;`, RFC 5322 §3.4). A group's
//!   members would need to be flattened into the list, and nothing sends one.
//!
//! Each of those is a property an imported `Email` simply does not carry, rather
//! than one it carries a guess in. A test that needs it seeds a message instead.

use jmap_proto::mail::EmailAddress;

/// How many characters of the body a `preview` carries — the same 64 a seeded
/// message gets, so a list of both does not visibly mix two kinds of message.
const PREVIEW: usize = 64;

/// A message, as far as this server reads one.
pub(crate) struct Message {
    /// Header fields in the order they appeared: name lowercased (RFC 5322 §1.2
    /// makes a field name case-insensitive), value unfolded and trimmed.
    fields: Vec<(String, String)>,
    /// Everything after the blank line, verbatim. Empty when the message is
    /// headers alone, which RFC 5322 §3.5 allows.
    body: String,
}

impl Message {
    /// Reads `source`, or answers `None` if it is not a message at all.
    ///
    /// `None` is what RFC 8621 §4.8's `invalidEmail` is for, so the bar is the
    /// one that separates "a message this server understands imperfectly" from
    /// "not a message": valid UTF-8 (RFC 6532 mail is UTF-8; anything else is
    /// not text), at least one header field, and a header block in which every
    /// line is a field or the continuation of one. A body it cannot make sense
    /// of is not a reason — a body is opaque bytes to this server either way.
    pub(crate) fn read(source: &[u8]) -> Option<Self> {
        let source = std::str::from_utf8(source).ok()?;
        let (headers, body) = split(source);

        let mut fields: Vec<(String, String)> = Vec::new();
        for line in headers.split("\r\n").flat_map(|line| line.split('\n')) {
            if line.is_empty() {
                continue;
            }
            // A line beginning with whitespace continues the field above it
            // (RFC 5322 §2.2.3); folding is where the CRLF goes, so unfolding
            // replaces it with the single space the fold interrupted.
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

    /// The first value of `name`, which must be given in lower case.
    ///
    /// First rather than last, and singular: RFC 5322 §3.6 makes `Subject`,
    /// `From` and the rest appear at most once in a valid message, and a message
    /// carrying two is one where the topmost is the one a reader sees.
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

    /// An address-list header as JMAP models it, or `None` if the header is not
    /// there. An empty list is not answered: RFC 8621 §4.1.2 has the property
    /// absent when the header is, and a `To: ` that parsed to nothing is a
    /// header this server could not read rather than a message to nobody.
    pub(crate) fn addresses(&self, name: &str) -> Option<Vec<EmailAddress>> {
        let addresses: Vec<EmailAddress> = split_addresses(self.field(name)?)
            .into_iter()
            .filter_map(address)
            .collect();
        (!addresses.is_empty()).then_some(addresses)
    }

    /// A `Message-ID`/`In-Reply-To`/`References` header as the bare ids JMAP
    /// carries — RFC 8621 §4.1.2.1 keeps them without the angle brackets.
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

    /// The start of the body as one line of text, or `None` for a message with
    /// no body.
    ///
    /// Whitespace collapsed, because a preview is shown on one line in a message
    /// list and the body's own line breaks would arrive in it as spaces anyway.
    /// The bytes are not decoded first: a quoted-printable or base64 body
    /// previews as its encoded form, which is the visible edge of having no MIME
    /// parser here.
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

/// The header block and the body, split at the first blank line.
///
/// A message with no blank line is all headers (RFC 5322 §3.5's body is
/// optional), which is why this cannot fail. Both line endings are accepted: a
/// client that serialized with bare LF has still handed over something every
/// mail store on earth would read.
fn split(source: &str) -> (&str, &str) {
    match source.find("\r\n\r\n") {
        Some(end) => (&source[..end], &source[end + 4..]),
        None => match source.find("\n\n") {
            Some(end) => (&source[..end], &source[end + 2..]),
            None => (source, ""),
        },
    }
}

/// Whether `byte` may appear in a field name: RFC 5322 §3.6.8's `ftext`, the
/// printable US-ASCII except the colon that ends the name.
fn is_field_name(byte: u8) -> bool {
    (33..=57).contains(&byte) || (59..=126).contains(&byte)
}

/// An address list at its top-level commas.
///
/// A comma inside a quoted display name or inside angle brackets is part of an
/// address rather than a separator — `"Doe, John" <j@example.com>` is one
/// recipient — so the split tracks both.
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

/// One address as JMAP models it, or `None` if there is nothing there.
///
/// `Name <local@domain>` and a bare `local@domain` are the two forms; a display
/// name is unquoted but not otherwise decoded (see the module header on RFC
/// 2047). Anything without an `@` is still answered rather than dropped: JMAP's
/// `email` is a string, and a mock that silently discarded a malformed address
/// would hide the malformed message rather than show it.
fn address(address: &str) -> Option<EmailAddress> {
    let address = address.trim();
    if address.is_empty() {
        return None;
    }
    let Some((name, rest)) = address.split_once('<') else {
        return Some(EmailAddress {
            name: None,
            email: address.to_owned(),
        });
    };
    let email = rest.trim_end().trim_end_matches('>').trim();
    if email.is_empty() {
        return None;
    }
    let name = name.trim().trim_matches('"').trim();
    Some(EmailAddress {
        name: (!name.is_empty()).then(|| name.to_owned()),
        email: email.to_owned(),
    })
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
        // No fields at all, a header block with a line that is not a field, a
        // continuation with nothing to continue, an empty field name, and bytes
        // that are not text.
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
