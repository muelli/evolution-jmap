// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Translating a Camel search expression into a JMAP `Email/query` filter.
//!
//! Evolution's message list, search bar and quick-search all reduce to one
//! call: `CamelFolderClass::search_by_expression`/`search_by_uids`, given a
//! string in the e-sexp grammar (`e-sexp.c`) built from
//! `mail/searchtypes.xml.in`'s rules, wrapped in `message-list.c`'s own
//! `(match-all ...)`. [`translate`] answers the question those two vfuncs
//! need answered before anything can go to the server: does this expression
//! mean exactly one thing in both grammars, or does answering it correctly
//! require Camel's own local search over the summary?
//!
//! The tokenizer here is not a general e-sexp parser — it does not need to
//! evaluate arithmetic, dates or user tags, only to recognise the shapes
//! Evolution actually generates and to fail safely (returning [`None`], the
//! same "stay local" answer as an untranslatable function) on anything else,
//! including malformed input.
//!
//! [`EmailQueryFilter`] only has "contains a substring" semantics (RFC 8621
//! §4.4.1's `text`/`body`/`from`/... fields, and the generic `header`
//! two-element form): `header-matches`, `header-starts-with`,
//! `header-ends-with`, `header-has-words`, `header-soundex` and
//! `header-exists` have no equivalent and are left untranslated, as is
//! `system-flag` (Camel's flag names and JMAP's `$`-prefixed keywords do not
//! line up one to one; RFC-SUPPORT.md tracks that as its own gap). A
//! sub-expression that cannot be translated fails the whole tree rather than
//! being dropped: dropping a child of `and` would query the server for more
//! than the user asked, and dropping one of `or` for less.

use jmap_proto::mail::EmailQueryFilter;
use jmap_proto::methods::Filter;

/// One node of the subset of e-sexp this module parses: lists, strings and
/// bare tokens (identifiers, numbers, `#t`/`#f`) — enough structure to walk
/// `(func "arg" (nested ...))` shapes without needing to evaluate them.
#[derive(Debug, Clone, PartialEq)]
enum SExpr {
    List(Vec<SExpr>),
    Ident(String),
    Str(String),
}

/// Parses `expression` and translates it to a JMAP filter tree, or `None` if
/// the expression is malformed or uses anything this module does not know an
/// exact JMAP equivalent for — in which case the caller should search
/// locally instead, the same fallback a genuine parse failure gets.
pub fn translate(expression: &str) -> Option<Filter<EmailQueryFilter>> {
    let (expr, rest) = parse_value(expression.trim_start())?;
    if !rest.trim().is_empty() {
        // Trailing garbage after the one top-level form the grammar allows.
        return None;
    }
    translate_bool(&expr)
}

/// Parses one value (a list, a string, or a bare token) off the front of
/// `input`, returning it with whatever is left unconsumed.
fn parse_value(input: &str) -> Option<(SExpr, &str)> {
    let input = skip_ignorable(input);
    let mut chars = input.char_indices();
    let (_, first) = chars.next()?;
    match first {
        '(' => parse_list(&input[first.len_utf8()..]),
        '"' | '\'' => parse_string(input, first),
        ')' => None,
        _ => Some(parse_token(input)),
    }
}

/// Skips whitespace and `;`-to-end-of-line comments, e-sexp's
/// `cset_skip_characters`/`cpair_comment_single`.
fn skip_ignorable(mut input: &str) -> &str {
    loop {
        let trimmed = input.trim_start();
        if let Some(rest) = trimmed.strip_prefix(';') {
            input = match rest.find('\n') {
                Some(pos) => &rest[pos + 1..],
                None => "",
            };
        } else {
            return trimmed;
        }
    }
}

/// Parses the elements of a list up to and including its closing `)`; the
/// opening `(` has already been consumed by the caller.
fn parse_list(mut input: &str) -> Option<(SExpr, &str)> {
    let mut items = Vec::new();
    loop {
        input = skip_ignorable(input);
        if let Some(rest) = input.strip_prefix(')') {
            return Some((SExpr::List(items), rest));
        }
        if input.is_empty() {
            return None;
        }
        let (item, rest) = parse_value(input)?;
        items.push(item);
        input = rest;
    }
}

/// Parses a `"..."`/`'...'` string, honouring `\\` as the one escape
/// e-sexp-generated strings need to round-trip (`e_sexp_encode_string`
/// backslash-escapes `\\` and the string's own quote character).
fn parse_string(input: &str, quote: char) -> Option<(SExpr, &str)> {
    let mut chars = input.char_indices();
    chars.next(); // the opening quote, already matched by the caller.
    let mut value = String::new();
    while let Some((idx, c)) = chars.next() {
        match c {
            '\\' => {
                let (_, escaped) = chars.next()?;
                value.push(escaped);
            }
            c if c == quote => {
                return Some((SExpr::Str(value), &input[idx + c.len_utf8()..]));
            }
            c => value.push(c),
        }
    }
    None // unterminated string
}

/// Parses a bare token: an identifier, a number, or `#t`/`#f` — anything run
/// of characters not whitespace, a paren or a quote. This module never needs
/// to tell those apart by type, only to carry the token's text: function
/// names are matched against as strings, and every other bare token appears
/// only in sub-expressions this module does not translate anyway.
fn parse_token(input: &str) -> (SExpr, &str) {
    let end = input
        .find(|c: char| c.is_whitespace() || c == '(' || c == ')')
        .unwrap_or(input.len());
    (SExpr::Ident(input[..end].to_owned()), &input[end..])
}

fn as_str(expr: &SExpr) -> Option<&str> {
    match expr {
        SExpr::Str(s) => Some(s),
        _ => None,
    }
}

/// Builds a `header`-contains condition for a well-known header, or falls
/// back to `EmailQueryFilter::header`'s generic name/value form for any
/// other header name; `""` is Camel's "any header" wildcard, which has no
/// JMAP equivalent.
fn header_contains(header: &str, value: &str) -> Option<Filter<EmailQueryFilter>> {
    let filter = if header.eq_ignore_ascii_case("From") {
        EmailQueryFilter::default().from(value)
    } else if header.eq_ignore_ascii_case("To") {
        EmailQueryFilter::default().to(value)
    } else if header.eq_ignore_ascii_case("Cc") {
        EmailQueryFilter::default().cc(value)
    } else if header.eq_ignore_ascii_case("Bcc") {
        EmailQueryFilter::default().bcc(value)
    } else if header.eq_ignore_ascii_case("Subject") {
        EmailQueryFilter::default().subject(value)
    } else if header.is_empty() {
        return None;
    } else {
        EmailQueryFilter::default().header(header, value)
    };
    Some(Filter::condition(filter))
}

/// Translates a boolean-valued e-sexp node. `and`/`or`/`not` and
/// `match-all` are structural and recurse; every leaf function this module
/// knows an exact JMAP equivalent for is matched by name; anything else
/// yields `None`.
fn translate_bool(expr: &SExpr) -> Option<Filter<EmailQueryFilter>> {
    let SExpr::List(items) = expr else {
        return None;
    };
    let [SExpr::Ident(func), args @ ..] = items.as_slice() else {
        return None;
    };
    match func.to_ascii_lowercase().as_str() {
        // message-list.c's own wrapper: apply the one child to every message.
        "match-all" => {
            let [inner] = args else { return None };
            translate_bool(inner)
        }
        "and" => {
            let conditions = args
                .iter()
                .map(translate_bool)
                .collect::<Option<Vec<_>>>()?;
            Some(Filter::and(conditions))
        }
        "or" => {
            let conditions = args
                .iter()
                .map(translate_bool)
                .collect::<Option<Vec<_>>>()?;
            Some(Filter::or(conditions))
        }
        "not" => {
            let [inner] = args else { return None };
            Some(Filter::not([translate_bool(inner)?]))
        }
        "header-contains" => {
            let [header, value] = args else { return None };
            header_contains(as_str(header)?, as_str(value)?)
        }
        "body-contains" => {
            let [value] = args else { return None };
            Some(Filter::condition(
                EmailQueryFilter::default().body(as_str(value)?),
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_contains_from() {
        assert_eq!(
            translate(r#"(header-contains "From" "alice")"#),
            Some(Filter::condition(EmailQueryFilter::default().from("alice")))
        );
    }

    #[test]
    fn header_contains_maps_each_well_known_header() {
        assert_eq!(
            translate(r#"(header-contains "To" "alice")"#),
            Some(Filter::condition(EmailQueryFilter::default().to("alice")))
        );
        assert_eq!(
            translate(r#"(header-contains "Cc" "alice")"#),
            Some(Filter::condition(EmailQueryFilter::default().cc("alice")))
        );
        assert_eq!(
            translate(r#"(header-contains "Bcc" "alice")"#),
            Some(Filter::condition(EmailQueryFilter::default().bcc("alice")))
        );
        assert_eq!(
            translate(r#"(header-contains "Subject" "hi")"#),
            Some(Filter::condition(EmailQueryFilter::default().subject("hi")))
        );
    }

    #[test]
    fn header_contains_unknown_header_uses_generic_field() {
        assert_eq!(
            translate(r#"(header-contains "X-Spam-Flag" "YES")"#),
            Some(Filter::condition(
                EmailQueryFilter::default().header("X-Spam-Flag", "YES")
            ))
        );
    }

    #[test]
    fn header_contains_any_header_is_untranslatable() {
        assert_eq!(translate(r#"(header-contains "" "alice")"#), None);
    }

    #[test]
    fn body_contains() {
        assert_eq!(
            translate(r#"(body-contains "quarterly report")"#),
            Some(Filter::condition(
                EmailQueryFilter::default().body("quarterly report")
            ))
        );
    }

    #[test]
    fn or_of_recipients_matches_searchtypes_recipients_rule() {
        assert_eq!(
            translate(r#"(or (header-contains "To" "x") (header-contains "Cc" "x"))"#),
            Some(Filter::or([
                Filter::condition(EmailQueryFilter::default().to("x")),
                Filter::condition(EmailQueryFilter::default().cc("x")),
            ]))
        );
    }

    #[test]
    fn and_of_subject_and_body() {
        assert_eq!(
            translate(r#"(and (header-contains "Subject" "s") (body-contains "b"))"#),
            Some(Filter::and([
                Filter::condition(EmailQueryFilter::default().subject("s")),
                Filter::condition(EmailQueryFilter::default().body("b")),
            ]))
        );
    }

    #[test]
    fn not_negates_its_one_child() {
        assert_eq!(
            translate(r#"(not (header-contains "From" "x"))"#),
            Some(Filter::not([Filter::condition(
                EmailQueryFilter::default().from("x")
            )]))
        );
    }

    #[test]
    fn match_all_wrapper_is_transparent() {
        assert_eq!(
            translate(r#"(match-all (header-contains "Subject" "s"))"#),
            Some(Filter::condition(EmailQueryFilter::default().subject("s")))
        );
    }

    #[test]
    fn unsupported_function_is_untranslatable() {
        assert_eq!(translate(r#"(header-matches "From" "x")"#), None);
        assert_eq!(translate(r#"(system-flag "Deleted")"#), None);
    }

    #[test]
    fn one_untranslatable_child_fails_the_whole_and() {
        assert_eq!(
            translate(r#"(and (header-contains "From" "x") (header-starts-with "Subject" "y"))"#),
            None
        );
    }

    #[test]
    fn one_untranslatable_child_fails_the_whole_or() {
        assert_eq!(
            translate(r#"(or (header-contains "From" "x") (system-flag "Deleted"))"#),
            None
        );
    }

    #[test]
    fn malformed_expression_is_untranslatable() {
        assert_eq!(translate(r#"(and (header-contains "From" "x")"#), None);
        assert_eq!(translate(r#"(header-contains "From" "unterminated)"#), None);
        assert_eq!(translate(""), None);
    }

    #[test]
    fn trailing_garbage_is_untranslatable() {
        assert_eq!(
            translate(r#"(header-contains "From" "x") (body-contains "y")"#),
            None
        );
    }
}
