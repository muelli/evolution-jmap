// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Substituting values into the session's URL templates.
//!
//! RFC 8620 §6.1 and §6.2 describe `uploadUrl` and `downloadUrl` as templates
//! with named variables, and require that "the client MUST URI-encode" each
//! value it puts in one. That obligation is not decoration. Every value
//! substituted here arrives from somewhere else: `accountId` and `blobId` come
//! off the wire — from the session document and from `Email/get` — and
//! [`jmap_proto::Id`] is a newtype over `String` with no grammar check, so a
//! `#` truncates the URL at a fragment, a `?` opens a query string and a `/`
//! invents a path segment. `name` is a caller's label, which for this crate is
//! a message uid. In each case the unencoded string does not address the blob
//! it was read from; it addresses a different URL, chosen by whoever wrote the
//! value.
//!
//! The escaping is the strictest of the RFC 3986 sets on purpose. A template
//! may place a variable in a path segment (`/download/{accountId}/{blobId}`)
//! or in a query value (`?accept={type}`), and this crate does not parse the
//! template to find out which; percent-encoding everything outside
//! §2.3's unreserved set is the one answer that is correct in both.

/// Percent-encode a value for substitution into a URL template.
///
/// Unreserved characters (RFC 3986 §2.3: `ALPHA / DIGIT / "-" / "." / "_" /
/// "~"`) pass through; every other byte becomes `%XX`. Encoding by byte rather
/// than by `char` is what makes non-ASCII right: `%` escapes octets, so a
/// multi-byte UTF-8 character becomes one escape per byte.
pub(crate) fn encode_template_value(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push('%');
                encoded.push(hex_digit(byte >> 4));
                encoded.push(hex_digit(byte & 0x0f));
            }
        }
    }
    encoded
}

/// One nibble as an uppercase hex digit, which is the case RFC 3986 §2.1 says
/// to produce (both are legal to read; only one is canonical to write).
fn hex_digit(nibble: u8) -> char {
    char::from(match nibble {
        0..=9 => b'0' + nibble,
        _ => b'A' + (nibble - 10),
    })
}

/// Replace `url`'s scheme and authority with `origin`'s, keeping its path and
/// query untouched.
///
/// Backs [`crate::ClientBuilder::rebase_urls_to_origin`]: a real deployment's
/// session document is authoritative about the *path* its endpoints live at,
/// but a client that reached the session through a different scheme/host
/// than the document names — a reverse proxy, NAT boundary, or a configured
/// public hostname the client cannot route to — needs the option to keep
/// talking to the origin that worked rather than the one the server states.
pub(crate) fn rebase_origin(url: &str, origin: &str) -> String {
    let path_and_beyond = match url.find("://") {
        Some(scheme_end) => {
            let after_scheme = scheme_end + 3;
            match url[after_scheme..].find('/') {
                Some(slash) => &url[after_scheme + slash..],
                None => "",
            }
        }
        None => url,
    };
    format!("{}{path_and_beyond}", origin.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::{encode_template_value, rebase_origin};

    #[test]
    fn unreserved_characters_pass_through() {
        assert_eq!(
            encode_template_value("Abc-123_x.y~z"),
            "Abc-123_x.y~z",
            "an ordinary id must not be made unreadable"
        );
    }

    #[test]
    fn the_characters_that_reshape_a_url_are_escaped() {
        assert_eq!(encode_template_value("a#b"), "a%23b");
        assert_eq!(encode_template_value("a?b"), "a%3Fb");
        assert_eq!(encode_template_value("a/b"), "a%2Fb");
        assert_eq!(encode_template_value("a b"), "a%20b");
        assert_eq!(encode_template_value("a%b"), "a%25b");
        assert_eq!(encode_template_value(".."), "..");
    }

    #[test]
    fn a_reserved_character_in_a_query_position_is_escaped_too() {
        // `&` and `=` are legal in a path segment and would split a query
        // string; the strict set means one encoder serves both positions.
        assert_eq!(encode_template_value("a&b=c"), "a%26b%3Dc");
        assert_eq!(encode_template_value("application/octet-stream"), {
            "application%2Foctet-stream"
        });
    }

    #[test]
    fn a_multibyte_character_becomes_one_escape_per_octet() {
        assert_eq!(encode_template_value("ä"), "%C3%A4");
        assert_eq!(encode_template_value("日"), "%E6%97%A5");
    }

    #[test]
    fn rebase_origin_keeps_the_path_and_swaps_the_scheme_and_authority() {
        assert_eq!(
            rebase_origin("https://example.com/jmap", "http://10.0.0.5:8080"),
            "http://10.0.0.5:8080/jmap"
        );
    }

    #[test]
    fn rebase_origin_keeps_a_template_with_query_and_braces_intact() {
        assert_eq!(
            rebase_origin(
                "https://example.com/download/{accountId}/{blobId}/{name}",
                "http://127.0.0.1:9"
            ),
            "http://127.0.0.1:9/download/{accountId}/{blobId}/{name}"
        );
    }

    #[test]
    fn rebase_origin_strips_a_trailing_slash_from_the_new_origin() {
        assert_eq!(
            rebase_origin("https://example.com/jmap", "http://10.0.0.5:8080/"),
            "http://10.0.0.5:8080/jmap"
        );
    }
}
