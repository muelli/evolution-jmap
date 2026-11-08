// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A local, offline syntax check of a Sieve script (item 171 step (b)).
//!
//! `SieveScript/validate` (RFC 9661 Section 2.6) asks the server, which
//! uploads the whole script as a blob and counts against the server's own
//! upload rate limit: typing one character per server round trip answers
//! HTTP 429 in practice (seen live on `stalwart-runner`, 2026-10-31). This
//! module runs the same parser locally instead, so the text-mode editor can
//! show a syntax error while the user is still typing. The Check button
//! stays the authoritative check: it also catches a capability or limit the
//! account's server enforces that a bare parse cannot know about.

use sieve::Compiler;

/// One syntax problem the local parser found.
#[derive(Debug, PartialEq, Eq)]
pub struct SieveProblem {
    /// One-based line number, matching [`sieve::compiler::CompileError`].
    pub line: usize,
    /// One-based column on that line.
    pub column: usize,
    /// A human-readable description, for the error's tooltip.
    pub message: String,
}

/// Checks `source` and returns its first syntax problem, if any. `None`
/// means the local parser accepts the script; it does not mean the
/// server will, since this check knows nothing of the account's declared
/// capabilities or server-side limits.
pub fn find_problem(source: &str) -> Option<SieveProblem> {
    match Compiler::new().compile(source.as_bytes()) {
        Ok(_) => None,
        Err(error) => Some(SieveProblem {
            line: error.line_num(),
            column: error.line_pos(),
            message: error.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_script_has_no_problem() {
        assert_eq!(find_problem(""), None);
    }

    #[test]
    fn keep_is_a_valid_script() {
        assert_eq!(find_problem("keep;\n"), None);
    }

    #[test]
    fn a_managed_rule_script_is_a_valid_script() {
        assert_eq!(
            find_problem(
                "require [\"fileinto\"];\n\
                 if header :contains \"subject\" \"invoice\" {\n\
                 \tfileinto \"Invoices\";\n\
                 }\n"
            ),
            None
        );
    }

    #[test]
    fn an_unterminated_block_is_a_problem_with_a_message() {
        let problem = find_problem("if true {\n  keep;\n").expect("should not parse");
        assert!(!problem.message.is_empty());
    }

    #[test]
    fn a_missing_semicolon_is_reported_on_its_own_line() {
        // The missing ";" after "keep" is only discoverable once the parser
        // reaches the next token, the "stop" on line 2.
        let problem = find_problem("keep\nstop;\n").expect("should not parse");
        assert_eq!(problem.line, 2);
    }
}
