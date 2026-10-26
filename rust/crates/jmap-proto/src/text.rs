// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Text encoding, line folding, and URI helpers for protocol formats.
//!
//! Provides common text processing functions shared across calendar,
//! contact, and protocol serialization:
//!
//! - Line folding adhering to RFC 5545 §3.1 and RFC 2426 §2.6 (75-octet limit)
//! - Caret encoding and decoding adhering to RFC 6868 §3
//! - URI scheme syntax validation adhering to RFC 3986 §3.1
//! - JSON Pointer segment escaping adhering to RFC 6901 §3

/// The standard line length limit in octets before folding, excluding line break (RFC 5545 §3.1, RFC 2426 §2.6).
pub const MAX_LINE_OCTETS: usize = 75;

/// Folds physical lines longer than [`MAX_LINE_OCTETS`] using CRLF and a space (`\r\n `).
///
/// Cuts at UTF-8 character boundaries and does not split an escaped pair
/// (an odd run of backslashes before the cut).
pub fn fold_overlong_lines(text: String) -> String {
    fold_overlong_lines_at(text, MAX_LINE_OCTETS)
}

/// Folds physical lines longer than `max_octets` using CRLF and a space (`\r\n `).
pub fn fold_overlong_lines_at(text: String, max_octets: usize) -> String {
    if text.split("\r\n").all(|line| line.len() <= max_octets) {
        return text;
    }
    let mut out = String::with_capacity(text.len() + 16);
    for (index, line) in text.split("\r\n").enumerate() {
        if index > 0 {
            out.push_str("\r\n");
        }
        let mut rest = line;
        let mut budget = max_octets;
        while rest.len() > budget {
            if rest.is_empty() {
                break;
            }
            let mut cut = budget;
            while cut > 0 && !rest.is_char_boundary(cut) {
                cut -= 1;
            }
            if rest[..cut]
                .bytes()
                .rev()
                .take_while(|&byte| byte == b'\\')
                .count()
                % 2
                == 1
            {
                cut -= 1;
            }
            if cut == 0 {
                cut = (1..=rest.len())
                    .find(|&i| rest.is_char_boundary(i))
                    .unwrap_or(rest.len());
            }
            out.push_str(&rest[..cut]);
            out.push_str("\r\n ");
            rest = &rest[cut..];
            budget = max_octets.saturating_sub(1);
        }
        out.push_str(rest);
    }
    out
}

/// Applies RFC 6868 §3 caret escapes to a parameter value.
///
/// Encodes `^` to `^^`, `"` to `^'`, and line breaks (`\n`, `\r\n`, or lone `\r`) to `^n`.
pub fn caret_encoded(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '^' => out.push_str("^^"),
            '"' => out.push_str("^'"),
            '\n' => out.push_str("^n"),
            '\r' => {
                chars.next_if_eq(&'\n');
                out.push_str("^n");
            }
            _ => out.push(ch),
        }
    }
    out
}

/// Resolves RFC 6868 §3 caret escapes in a parameter value.
///
/// Decodes `^^` to `^`, `^'` to `"`, and `^n` or `^N` to `\n`.
/// A caret before any other character remains unchanged.
pub fn caret_decoded(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        let decoded = match (ch, chars.peek()) {
            ('^', Some('^')) => '^',
            ('^', Some('\'')) => '"',
            ('^', Some('n' | 'N')) => '\n',
            _ => {
                out.push(ch);
                continue;
            }
        };
        chars.next();
        out.push(decoded);
    }
    out
}

/// Returns true if `scheme` is a valid RFC 3986 §3.1 URI scheme name.
///
/// A scheme begins with an ASCII letter, followed by any combination of ASCII
/// letters, digits, plus (`+`), hyphen (`-`), or period (`.`).
pub fn is_valid_scheme(scheme: &str) -> bool {
    let mut characters = scheme.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
}

/// Returns the URI scheme name when `value` begins with an RFC 3986 §3.1 scheme followed by a colon.
pub fn uri_scheme(value: &str) -> Option<&str> {
    let (scheme, _) = value.split_once(':')?;
    is_valid_scheme(scheme).then_some(scheme)
}

/// Splits `value` into `(scheme, rest)` when `value` begins with an RFC 3986 §3.1 scheme followed by a colon.
pub fn split_uri(value: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = value.split_once(':')?;
    is_valid_scheme(scheme).then_some((scheme, rest))
}

/// Returns true when `value` begins with an RFC 3986 §3.1 scheme followed by a colon.
pub fn has_uri_scheme(value: &str) -> bool {
    uri_scheme(value).is_some()
}

/// Escapes a map key for use as one segment of a JSON Pointer (RFC 6901 §3).
///
/// Escape `~` first, or the `~1` that replaces a `/` becomes `~01`.
pub fn escape_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}
