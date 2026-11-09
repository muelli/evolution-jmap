// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Completion candidates for the text-mode editor (item 171 step (c)).
//!
//! Pure data and string logic only: no GTK here. A later step reads
//! [`candidates`] from a `GtkSourceCompletion` popup or a custom one.
//!
//! Each candidate names the Sieve extension (RFC 5228 §3.2 `require`) it
//! needs, if any. A candidate with no extension is always offered: it is
//! part of the Sieve base spec (RFC 5228), needing no `require` line. The
//! account's own `sieve_extensions` capability list
//! ([`jmap_proto::sieve::SieveCapability::sieve_extensions`]) gates the
//! rest, so the editor never offers a command the account's server does
//! not support.

/// What a candidate completes: a command, a test, a tagged argument, or an
/// extension name (inside a `require` line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    Command,
    Test,
    TaggedArgument,
    Extension,
}

/// One completion candidate: its text, its kind, and the extension that
/// must be in the account's `sieve_extensions` list for it to apply (`None`
/// for RFC 5228 base syntax, needing no `require` line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub text: &'static str,
    pub kind: CandidateKind,
    pub requires: Option<&'static str>,
}

const fn base(text: &'static str, kind: CandidateKind) -> Candidate {
    Candidate {
        text,
        kind,
        requires: None,
    }
}

const fn needs(text: &'static str, kind: CandidateKind, extension: &'static str) -> Candidate {
    Candidate {
        text,
        kind,
        requires: Some(extension),
    }
}

/// Every candidate this editor knows, RFC base syntax first, then one
/// group per extension. Extension names are the IANA Sieve Extensions
/// registry names, the exact strings a `require` line uses.
const ALL: &[Candidate] = &[
    // RFC 5228 control and action commands: no `require` needed.
    base("if", CandidateKind::Command),
    base("elsif", CandidateKind::Command),
    base("else", CandidateKind::Command),
    base("require", CandidateKind::Command),
    base("stop", CandidateKind::Command),
    base("keep", CandidateKind::Command),
    base("discard", CandidateKind::Command),
    base("redirect", CandidateKind::Command),
    // RFC 5228 test commands: no `require` needed.
    base("address", CandidateKind::Test),
    base("allof", CandidateKind::Test),
    base("anyof", CandidateKind::Test),
    base("not", CandidateKind::Test),
    base("exists", CandidateKind::Test),
    base("header", CandidateKind::Test),
    base("size", CandidateKind::Test),
    base("true", CandidateKind::Test),
    base("false", CandidateKind::Test),
    // RFC 5228 match-types and address-parts: no `require` needed.
    base(":is", CandidateKind::TaggedArgument),
    base(":contains", CandidateKind::TaggedArgument),
    base(":matches", CandidateKind::TaggedArgument),
    base(":comparator", CandidateKind::TaggedArgument),
    base(":all", CandidateKind::TaggedArgument),
    base(":localpart", CandidateKind::TaggedArgument),
    base(":domain", CandidateKind::TaggedArgument),
    base(":over", CandidateKind::TaggedArgument),
    base(":under", CandidateKind::TaggedArgument),
    // RFC 5228 §3.2: "fileinto".
    needs("fileinto", CandidateKind::Command, "fileinto"),
    // RFC 5228 §5.4: "envelope".
    needs("envelope", CandidateKind::Test, "envelope"),
    // RFC 5230: "vacation".
    needs("vacation", CandidateKind::Command, "vacation"),
    needs(":days", CandidateKind::TaggedArgument, "vacation"),
    needs(":subject", CandidateKind::TaggedArgument, "vacation"),
    needs(":addresses", CandidateKind::TaggedArgument, "vacation"),
    // RFC 6131: "vacation-seconds".
    needs(
        ":seconds",
        CandidateKind::TaggedArgument,
        "vacation-seconds",
    ),
    // RFC 5232: "imap4flags".
    needs("setflag", CandidateKind::Command, "imap4flags"),
    needs("addflag", CandidateKind::Command, "imap4flags"),
    needs("removeflag", CandidateKind::Command, "imap4flags"),
    needs("hasflag", CandidateKind::Test, "imap4flags"),
    needs(":flags", CandidateKind::TaggedArgument, "imap4flags"),
    // RFC 3894: "copy".
    needs(":copy", CandidateKind::TaggedArgument, "copy"),
    // RFC 5173: "body".
    needs("body", CandidateKind::Test, "body"),
    // RFC 5183: "environment".
    needs("environment", CandidateKind::Test, "environment"),
    // RFC 5229: "variables".
    needs("set", CandidateKind::Command, "variables"),
    needs(":length", CandidateKind::TaggedArgument, "variables"),
    // RFC 5231: "relational".
    needs(":count", CandidateKind::TaggedArgument, "relational"),
    needs(":value", CandidateKind::TaggedArgument, "relational"),
    // RFC 5233: "subaddress".
    needs(":user", CandidateKind::TaggedArgument, "subaddress"),
    needs(":detail", CandidateKind::TaggedArgument, "subaddress"),
    // RFC 5260: "date" and "index".
    needs("date", CandidateKind::Test, "date"),
    needs("currentdate", CandidateKind::Test, "date"),
    needs(":index", CandidateKind::TaggedArgument, "index"),
    needs(":last", CandidateKind::TaggedArgument, "index"),
    // RFC 5293: "editheader".
    needs("addheader", CandidateKind::Command, "editheader"),
    needs("deleteheader", CandidateKind::Command, "editheader"),
    // RFC 5429: "reject" and "ereject".
    needs("reject", CandidateKind::Command, "reject"),
    needs("ereject", CandidateKind::Command, "ereject"),
    // RFC 5435: "enotify".
    needs("notify", CandidateKind::Command, "enotify"),
    needs("validnotifymethod", CandidateKind::Test, "enotify"),
    // RFC 5463: "ihave".
    needs("ihave", CandidateKind::Command, "ihave"),
    needs("error", CandidateKind::Command, "ihave"),
    // RFC 5490: "mailbox".
    needs("mailboxexists", CandidateKind::Test, "mailbox"),
    needs(":create", CandidateKind::TaggedArgument, "mailbox"),
    // RFC 5490: "mboxmetadata" and "servermetadata".
    needs("metadata", CandidateKind::Test, "mboxmetadata"),
    needs("metadataexists", CandidateKind::Test, "mboxmetadata"),
    needs("servermetadata", CandidateKind::Test, "servermetadata"),
    needs(
        "servermetadataexists",
        CandidateKind::Test,
        "servermetadata",
    ),
    // RFC 5703: "mime", "foreverypart", "replace", "enclose", "extracttext".
    needs(":mime", CandidateKind::TaggedArgument, "mime"),
    needs("foreverypart", CandidateKind::Command, "foreverypart"),
    needs("replace", CandidateKind::Command, "replace"),
    needs("enclose", CandidateKind::Command, "enclose"),
    needs("extracttext", CandidateKind::Command, "extracttext"),
    // RFC 6134: "extlists".
    needs("validextlist", CandidateKind::Test, "extlists"),
    needs(":list", CandidateKind::TaggedArgument, "extlists"),
    // RFC 6558: "spamtest", "spamtestplus" and "virustest".
    needs("spamtest", CandidateKind::Test, "spamtest"),
    needs("virustest", CandidateKind::Test, "virustest"),
    // RFC 6609: "include".
    needs("include", CandidateKind::Command, "include"),
    needs("global", CandidateKind::Command, "include"),
    needs("return", CandidateKind::Command, "include"),
    // RFC 7352: "duplicate".
    needs("duplicate", CandidateKind::Test, "duplicate"),
    // RFC 9042: "mailboxid".
    needs("mailboxidexists", CandidateKind::Test, "mailboxid"),
    needs(":mailboxid", CandidateKind::TaggedArgument, "mailboxid"),
    // RFC 9042: "special-use".
    needs("specialuseexists", CandidateKind::Test, "special-use"),
    needs(":specialuse", CandidateKind::TaggedArgument, "special-use"),
];

/// The extension name itself, for a `require` line's own string literal.
/// One entry per extension [`ALL`] gates a command, test or tagged
/// argument on; `requires` names the same extension, so the gate in
/// [`candidates`] offers it only when the server supports it.
const EXTENSIONS: &[Candidate] = &[
    needs("fileinto", CandidateKind::Extension, "fileinto"),
    needs("envelope", CandidateKind::Extension, "envelope"),
    needs("vacation", CandidateKind::Extension, "vacation"),
    needs(
        "vacation-seconds",
        CandidateKind::Extension,
        "vacation-seconds",
    ),
    needs("imap4flags", CandidateKind::Extension, "imap4flags"),
    needs("copy", CandidateKind::Extension, "copy"),
    needs("body", CandidateKind::Extension, "body"),
    needs("environment", CandidateKind::Extension, "environment"),
    needs("variables", CandidateKind::Extension, "variables"),
    needs("relational", CandidateKind::Extension, "relational"),
    needs("subaddress", CandidateKind::Extension, "subaddress"),
    needs("date", CandidateKind::Extension, "date"),
    needs("index", CandidateKind::Extension, "index"),
    needs("editheader", CandidateKind::Extension, "editheader"),
    needs("reject", CandidateKind::Extension, "reject"),
    needs("ereject", CandidateKind::Extension, "ereject"),
    needs("enotify", CandidateKind::Extension, "enotify"),
    needs("ihave", CandidateKind::Extension, "ihave"),
    needs("mailbox", CandidateKind::Extension, "mailbox"),
    needs("mboxmetadata", CandidateKind::Extension, "mboxmetadata"),
    needs("servermetadata", CandidateKind::Extension, "servermetadata"),
    needs("mime", CandidateKind::Extension, "mime"),
    needs("foreverypart", CandidateKind::Extension, "foreverypart"),
    needs("replace", CandidateKind::Extension, "replace"),
    needs("enclose", CandidateKind::Extension, "enclose"),
    needs("extracttext", CandidateKind::Extension, "extracttext"),
    needs("extlists", CandidateKind::Extension, "extlists"),
    needs("spamtest", CandidateKind::Extension, "spamtest"),
    needs("virustest", CandidateKind::Extension, "virustest"),
    needs("include", CandidateKind::Extension, "include"),
    needs("duplicate", CandidateKind::Extension, "duplicate"),
    needs("mailboxid", CandidateKind::Extension, "mailboxid"),
    needs("special-use", CandidateKind::Extension, "special-use"),
];

/// Returns every candidate `prefix` starts with (case-insensitive),
/// restricted to RFC 5228 base syntax plus whatever `enabled_extensions`
/// names. Order follows the internal table; callers that want an
/// alphabetical popup list sort the result themselves.
pub fn candidates<'a>(
    prefix: &str,
    enabled_extensions: &'a [String],
) -> impl Iterator<Item = &'static Candidate> + 'a {
    let prefix = prefix.to_ascii_lowercase();
    ALL.iter()
        .chain(EXTENSIONS.iter())
        .filter(move |candidate| {
            candidate.text.to_ascii_lowercase().starts_with(&prefix)
                && match candidate.requires {
                    None => true,
                    Some(extension) => enabled_extensions.iter().any(|e| e == extension),
                }
        })
}

/// Returns the partial word the user is typing, the run of identifier
/// characters (and a leading `:` for a tagged argument) ending at
/// `text`'s own end. `text` is everything before the cursor on the
/// current line; the caller passes only that slice.
pub fn word_under_cursor(text: &str) -> &str {
    let mut start = text.len();
    let mut chars = text.char_indices().rev().peekable();
    while let Some(&(index, ch)) = chars.peek() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            start = index;
            chars.next();
        } else {
            break;
        }
    }
    if start > 0 && text[..start].ends_with(':') {
        start -= 1;
    }
    &text[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(prefix: &str, extensions: &[&str]) -> Vec<&'static str> {
        let extensions: Vec<String> = extensions.iter().map(|s| s.to_string()).collect();
        candidates(prefix, &extensions).map(|c| c.text).collect()
    }

    #[test]
    fn a_base_command_is_offered_with_no_extensions_enabled() {
        assert!(names("ke", &[]).contains(&"keep"));
        assert!(names("sto", &[]).contains(&"stop"));
    }

    #[test]
    fn a_base_tagged_argument_is_offered_with_no_extensions_enabled() {
        assert!(names(":con", &[]).contains(&":contains"));
    }

    #[test]
    fn an_extension_gated_command_is_withheld_when_disabled() {
        assert!(!names("vaca", &[]).contains(&"vacation"));
    }

    #[test]
    fn an_extension_gated_command_is_offered_once_its_extension_is_enabled() {
        assert!(names("vaca", &["vacation"]).contains(&"vacation"));
    }

    #[test]
    fn a_disabled_sibling_extension_does_not_leak_its_candidates() {
        // "imap4flags" enabled must not also unlock "variables"'s "set".
        assert!(!names("se", &["imap4flags"]).contains(&"set"));
        assert!(names("se", &["imap4flags"]).contains(&"setflag"));
    }

    #[test]
    fn matching_is_case_insensitive_on_the_prefix() {
        assert!(names("KE", &[]).contains(&"keep"));
    }

    #[test]
    fn an_unmatched_prefix_offers_nothing() {
        assert!(names("zzz", &["vacation", "imap4flags"]).is_empty());
    }

    #[test]
    fn word_under_cursor_finds_a_plain_identifier() {
        assert_eq!(word_under_cursor("if header :con"), ":con");
        assert_eq!(word_under_cursor("if head"), "head");
    }

    #[test]
    fn word_under_cursor_is_empty_right_after_whitespace() {
        assert_eq!(word_under_cursor("if "), "");
    }

    #[test]
    fn word_under_cursor_is_empty_on_an_empty_line() {
        assert_eq!(word_under_cursor(""), "");
    }

    #[test]
    fn word_under_cursor_stops_at_a_quote() {
        assert_eq!(word_under_cursor("header :contains \"sub"), "sub");
    }

    #[test]
    fn every_extension_gate_in_all_has_a_matching_require_candidate() {
        // Every `requires` value in `ALL` must also exist as an
        // `EXTENSIONS` entry of the same name, or `require "<name>"`
        // itself would never be offered.
        for candidate in ALL {
            if let Some(extension) = candidate.requires {
                assert!(
                    EXTENSIONS
                        .iter()
                        .any(|e| e.text == extension && e.requires == Some(extension)),
                    "{} needs {:?}, which has no require candidate",
                    candidate.text,
                    extension
                );
            }
        }
    }

    #[test]
    fn a_require_candidate_is_offered_only_once_its_own_extension_is_enabled() {
        let enabled: Vec<String> = vec!["vacation".to_string()];
        assert!(
            candidates("vaca", &enabled)
                .any(|c| c.text == "vacation" && c.kind == CandidateKind::Extension)
        );
        assert!(!candidates("vaca", &[]).any(|c| c.kind == CandidateKind::Extension));
    }
}
