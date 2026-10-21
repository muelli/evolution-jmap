// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Translates a Camel search expression into a JMAP `Email/query` filter.
//!
//! Evolution sends its searches to `CamelFolderClass::search_by_expression` and
//! `search_by_uids` as e-sexp strings (`e-sexp.c`). [`translate`] decides if an
//! expression has exactly one meaning in JMAP, or if Camel must search locally.
//!
//! This is not a full e-sexp parser. It accepts the shapes that Evolution makes
//! and gives [`None`] ("search locally") for all other input, also malformed input.
//!
//! [`EmailQueryFilter`] has only substring semantics (RFC 8621 §4.4.1). Thus
//! `header-matches`, `header-starts-with`, `header-ends-with`,
//! `header-has-words`, `header-soundex` and `header-exists` stay untranslated.
//!
//! `system-flag` translates the seven Camel flag names (`camel_system_flag` in
//! `camel-folder-summary.c`) that have an exact JMAP equivalent. It does not
//! translate `deleted` (JMAP removes a message, it has no flag for this),
//! `secure` (local S/MIME state), `junklearn` (junk filter training state) or
//! an unknown name.
//!
//! An untranslatable sub-expression fails the full tree. If a child of `and`
//! is removed, the query asks for more messages; for `or`, it asks for fewer.

use jmap_proto::mail::{EmailQueryFilter, keyword};
use jmap_proto::methods::Filter;

/// One node of the e-sexp subset that this module parses: lists, strings and
/// bare tokens (identifiers, numbers, `#t`/`#f`). This module does not
/// evaluate the nodes.
#[derive(Debug, Clone, PartialEq)]
enum SExpr {
    List(Vec<SExpr>),
    Ident(String),
    Str(String),
}

/// Parses `expression` and translates it to a JMAP filter tree.
///
/// Returns `None` if the expression is malformed or has no exact JMAP
/// equivalent. Then the caller must search locally.
pub fn translate(expression: &str) -> Option<Filter<EmailQueryFilter>> {
    let (expr, rest) = parse_value(expression.trim_start(), 0)?;
    if !rest.trim().is_empty() {
        // The grammar allows only one top-level form.
        return None;
    }
    translate_bool(&expr)
}

/// The maximum nesting depth of `(`. The parser refuses a deeper expression.
///
/// [`parse_value`] and [`parse_list`] recurse once for each `(`. Without a
/// limit, deep input overflows the stack, and this stops the full Evolution
/// process. The value is the same as the `parentId` chain limit of the
/// `folder` module of `jmap-mail-sync`. Evolution makes at most four levels:
/// `(match-all (and (or …) …))`.
const MAX_DEPTH: usize = 64;

/// Parses one value (a list, a string, or a bare token) at the start of
/// `input`, and returns it with the remaining input.
///
/// `depth` is the number of lists that contain this value. A list deeper than
/// [`MAX_DEPTH`] gives `None`.
fn parse_value(input: &str, depth: usize) -> Option<(SExpr, &str)> {
    let input = skip_ignorable(input);
    let mut chars = input.char_indices();
    let (_, first) = chars.next()?;
    match first {
        '(' if depth >= MAX_DEPTH => None,
        '(' => parse_list(&input[first.len_utf8()..], depth + 1),
        '"' | '\'' => parse_string(input, first),
        ')' => None,
        _ => Some(parse_token(input)),
    }
}

/// Skips whitespace and `;` comments to the end of the line, as
/// `cset_skip_characters`/`cpair_comment_single` of e-sexp do.
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

/// Parses the elements of a list up to and including its closing `)`.
///
/// The caller has already consumed the opening `(`, and `depth` includes it.
fn parse_list(mut input: &str, depth: usize) -> Option<(SExpr, &str)> {
    let mut items = Vec::new();
    loop {
        input = skip_ignorable(input);
        if let Some(rest) = input.strip_prefix(')') {
            return Some((SExpr::List(items), rest));
        }
        if input.is_empty() {
            return None;
        }
        let (item, rest) = parse_value(input, depth)?;
        items.push(item);
        input = rest;
    }
}

/// Parses a `"..."`/`'...'` string. A backslash escapes the next character.
///
/// `e_sexp_encode_string` puts a backslash before `\\` and before the quote
/// character of the string.
fn parse_string(input: &str, quote: char) -> Option<(SExpr, &str)> {
    let mut chars = input.char_indices();
    chars.next(); // The caller has already matched the opening quote.
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
    None // The string has no closing quote.
}

/// Parses a bare token: an identifier, a number, or `#t`/`#f`. The token ends
/// at whitespace or a parenthesis.
///
/// The token type is not necessary. Function names are compared as text, and
/// other bare tokens occur only in sub-expressions that stay untranslated.
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

/// Builds a header-contains condition. A well-known header gets its own
/// field, and other headers use the generic `EmailQueryFilter::header` form.
///
/// `""` is the Camel "any header" wildcard. It has no JMAP equivalent.
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

/// Builds a condition for `(system-flag "<name>")` for the seven Camel system
/// flags with an exact JMAP equivalent.
///
/// Returns `None` for the other three Camel flags (see the module doc) and for
/// an unknown name. The comparison ignores case, as `camel_system_flag` does.
fn system_flag(name: &str) -> Option<Filter<EmailQueryFilter>> {
    let filter = if name.eq_ignore_ascii_case("answered") {
        EmailQueryFilter::default().has_keyword(keyword::ANSWERED)
    } else if name.eq_ignore_ascii_case("draft") {
        EmailQueryFilter::default().has_keyword(keyword::DRAFT)
    } else if name.eq_ignore_ascii_case("flagged") {
        EmailQueryFilter::default().has_keyword(keyword::FLAGGED)
    } else if name.eq_ignore_ascii_case("seen") {
        EmailQueryFilter::default().has_keyword(keyword::SEEN)
    } else if name.eq_ignore_ascii_case("junk") {
        EmailQueryFilter::default().has_keyword(keyword::JUNK)
    } else if name.eq_ignore_ascii_case("notjunk") {
        EmailQueryFilter::default().has_keyword(keyword::NOT_JUNK)
    } else if name.eq_ignore_ascii_case("attachments") {
        EmailQueryFilter::default().has_attachment(true)
    } else {
        return None;
    };
    Some(Filter::condition(filter))
}

/// Translates a boolean e-sexp node. `and`/`or`/`not` and `match-all`
/// recurse, and leaf functions with an exact JMAP equivalent match by name.
/// All other nodes give `None`.
fn translate_bool(expr: &SExpr) -> Option<Filter<EmailQueryFilter>> {
    let SExpr::List(items) = expr else {
        return None;
    };
    let [SExpr::Ident(func), args @ ..] = items.as_slice() else {
        return None;
    };
    match func.to_ascii_lowercase().as_str() {
        // The wrapper of message-list.c: it applies its one child to all messages.
        "match-all" => {
            let [inner] = args else { return None };
            translate_bool(inner)
        }
        "and" => {
            if args.is_empty() {
                return None;
            }
            let conditions = args
                .iter()
                .map(translate_bool)
                .collect::<Option<Vec<_>>>()?;
            Some(Filter::and(conditions))
        }
        "or" => {
            if args.is_empty() {
                return None;
            }
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
        "system-flag" => {
            let [name] = args else { return None };
            system_flag(as_str(name)?)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without [`MAX_DEPTH`], 100 000 `(` overflow the stack and stop the
    /// process. With it, the result is `None` ("search locally").
    #[test]
    fn a_run_of_open_parens_is_refused_rather_than_overflowing_the_stack() {
        assert_eq!(translate(&"(".repeat(100_000)), None);
        // A balanced expression of the same depth is also refused, because
        // the nesting uses the stack, not the missing `)`.
        let deep = format!("{}{}", "(and ".repeat(50_000), ")".repeat(50_000));
        assert_eq!(translate(&deep), None);
    }

    /// The limit refuses only deeper expressions. An expression at the limit
    /// still translates.
    #[test]
    fn nesting_up_to_the_limit_still_translates() {
        let leaf = r#"(header-contains "Subject" "s")"#;
        // `translate` counts the outermost list as depth 1. Thus MAX_DEPTH - 1
        // `not` wrappers and the list of the leaf are exactly at the limit.
        let at_limit = format!(
            "{}{leaf}{}",
            "(not ".repeat(MAX_DEPTH - 1),
            ")".repeat(MAX_DEPTH - 1)
        );
        assert!(
            translate(&at_limit).is_some(),
            "an expression at the limit must still translate"
        );

        let past_limit = format!(
            "{}{leaf}{}",
            "(not ".repeat(MAX_DEPTH),
            ")".repeat(MAX_DEPTH)
        );
        assert_eq!(translate(&past_limit), None, "one level past is refused");
    }

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
    }

    #[test]
    fn system_flag_maps_each_camel_flag_with_a_jmap_equivalent() {
        assert_eq!(
            translate(r#"(system-flag "Answered")"#),
            Some(Filter::condition(
                EmailQueryFilter::default().has_keyword(keyword::ANSWERED)
            ))
        );
        assert_eq!(
            translate(r#"(system-flag "Draft")"#),
            Some(Filter::condition(
                EmailQueryFilter::default().has_keyword(keyword::DRAFT)
            ))
        );
        assert_eq!(
            translate(r#"(system-flag "Flagged")"#),
            Some(Filter::condition(
                EmailQueryFilter::default().has_keyword(keyword::FLAGGED)
            ))
        );
        assert_eq!(
            translate(r#"(system-flag "Seen")"#),
            Some(Filter::condition(
                EmailQueryFilter::default().has_keyword(keyword::SEEN)
            ))
        );
        assert_eq!(
            translate(r#"(system-flag "Junk")"#),
            Some(Filter::condition(
                EmailQueryFilter::default().has_keyword(keyword::JUNK)
            ))
        );
        assert_eq!(
            translate(r#"(system-flag "NotJunk")"#),
            Some(Filter::condition(
                EmailQueryFilter::default().has_keyword(keyword::NOT_JUNK)
            ))
        );
        assert_eq!(
            translate(r#"(system-flag "Attachments")"#),
            Some(Filter::condition(
                EmailQueryFilter::default().has_attachment(true)
            ))
        );
    }

    #[test]
    fn system_flag_matches_camel_case_insensitively() {
        // camel_system_flag compares with g_ascii_strcasecmp.
        assert_eq!(
            translate(r#"(system-flag "SEEN")"#),
            Some(Filter::condition(
                EmailQueryFilter::default().has_keyword(keyword::SEEN)
            ))
        );
        assert_eq!(
            translate(r#"(system-flag "seen")"#),
            Some(Filter::condition(
                EmailQueryFilter::default().has_keyword(keyword::SEEN)
            ))
        );
    }

    #[test]
    fn system_flag_negates_through_not() {
        assert_eq!(
            translate(r#"(not (system-flag "Seen"))"#),
            Some(Filter::not([Filter::condition(
                EmailQueryFilter::default().has_keyword(keyword::SEEN)
            )]))
        );
    }

    #[test]
    fn system_flag_names_with_no_jmap_equivalent_are_untranslatable() {
        // `deleted`: JMAP removes a message with a destroy or a mailbox
        // change. It has no flag for this.
        assert_eq!(translate(r#"(system-flag "Deleted")"#), None);
        // `secure`: local S/MIME verification state, not a server keyword.
        assert_eq!(translate(r#"(system-flag "Secure")"#), None);
        // `junklearn`: the junk filter's internal training-state flag.
        assert_eq!(translate(r#"(system-flag "JunkLearn")"#), None);
        // A name that is not in the Camel flag table.
        assert_eq!(translate(r#"(system-flag "NotARealFlag")"#), None);
    }

    #[test]
    fn unquoted_leaf_args_are_untranslatable() {
        // `parse_token` makes an `Ident` of unquoted text, and `as_str` refuses
        // it. The free-form expression mode of the Evolution search bar can
        // send this input.
        assert_eq!(translate(r#"(header-contains From "x")"#), None);
        assert_eq!(translate(r#"(header-contains "From" x)"#), None);
        assert_eq!(translate(r#"(body-contains foo)"#), None);
        assert_eq!(translate(r#"(system-flag Seen)"#), None);
    }

    #[test]
    fn empty_and_or_are_untranslatable() {
        // In e-sexp, `(and)` is true and `(or)` is false. RFC 8620 §5.5 does
        // not define a FilterOperator with an empty `conditions` array, so
        // `translate` refuses them.
        assert_eq!(translate("(and)"), None);
        assert_eq!(translate("(or)"), None);
        assert_eq!(
            translate(r#"(and (header-contains "From" "x") (or))"#),
            None
        );
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

    /// `skip_ignorable` must skip the newline that ends a comment. With an
    /// empty comment, the newline is at index 0, where a wrong offset underflows.
    #[test]
    fn empty_comment_right_before_a_newline_is_skipped() {
        assert_eq!(
            translate(";\n(body-contains \"x\")"),
            translate(r#"(body-contains "x")"#)
        );
    }

    /// A comment with text tests the same offset at a non-zero index.
    #[test]
    fn comment_with_text_before_an_expression_is_skipped() {
        assert_eq!(
            translate(";a comment\n(body-contains \"x\")"),
            translate(r#"(body-contains "x")"#)
        );
    }

    #[test]
    fn trailing_garbage_is_untranslatable() {
        assert_eq!(
            translate(r#"(header-contains "From" "x") (body-contains "y")"#),
            None
        );
    }
}
