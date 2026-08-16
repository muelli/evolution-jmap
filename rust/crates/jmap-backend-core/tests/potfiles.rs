// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `po/POTFILES.in` against the sources it claims to list.
//!
//! A message catalogue is built by running `xgettext` over the files named in
//! `POTFILES.in` and over nothing else. So a source that marks a string and is
//! not in that list is not a build error, not a warning, and not visible
//! anywhere: the string is simply absent from the `.pot`, no translator is ever
//! offered it, and it ships in English forever. The standing directive in
//! `docs/ROADMAP.md` asks for a check that catches exactly that, and this is it.
//!
//! It checks the list in both directions, because both are silent when wrong:
//!
//! - **Nothing marked is missing from the list.** The failure above.
//! - **Nothing in the list is stale.** A path that no longer exists makes
//!   `xgettext` fail outright, which is at least loud; a path that exists but no
//!   longer marks anything is worse, because it goes on working while the
//!   strings it used to contribute quietly leave the catalogue.
//!
//! ## What counts as a marked string
//!
//! A literal handed straight to one of the functions in
//! [`jmap_backend_core::i18n`] that gettext is told to key on — spelled
//! `N_(c"…")`, `translate(c"…")` or `translate_with(c"…", …)` in the source,
//! which is the form `xgettext --keyword` recognises. Deliberately a textual
//! match on the call site rather than anything cleverer: it is close to what
//! `xgettext` does, so it agrees with the tool on the cases where the tool is
//! the one being crude.
//!
//! Three consequences worth knowing before adding a string:
//!
//! - **A marker whose argument is not a literal is not a marked string** —
//!   `translate(NAME)` looks a message up but contributes nothing to extract,
//!   because the literal is wherever `NAME` was written. That is why the
//!   `c"` is part of the pattern.
//! - **Line comments do not count.** They are stripped before the match, so a
//!   doc comment may spell a marker out — this file's own module docs do —
//!   without putting the file in the list. `xgettext` skips comments too.
//! - **The literal need not be on the marker's own line.** `rustfmt` moves it
//!   to the next line as soon as the call is too wide, which is what a message
//!   long enough to be worth translating does. `xgettext` lexes rather than
//!   reads lines and does not care; a line-at-a-time match here would have
//!   quietly stopped recognising exactly the longest strings, so the whitespace
//!   between the two is skipped the way the lexer skips it.
//!
//! ## And a marked literal must be written on one line
//!
//! The list being right is necessary and not sufficient: `xgettext` still has
//! to read the literal the way `rustc` does, or the msgid in the catalogue is
//! not the msgid the program looks up and the translation is never found. The
//! two lex the same bytes and they differ on exactly one construct we can
//! write — a backslash at the end of a line. Rust drops the newline *and* the
//! indentation that follows it; C's line splicing drops only the newline. So
//!
//! ```text
//! c"This event repeats until %1$s, and the time zone it is in, \
//!   %2$s, …"
//! ```
//!
//! is looked up with one space before `%2$s` and extracted with fifteen. The
//! failure is completely silent: the extraction succeeds, the catalogue looks
//! right, the translator translates it, and the user still reads English.
//!
//! Hence the rule enforced here — no marked literal spans lines — rather than
//! the narrower "no *indentation* after the continuation". A column-zero
//! continuation would read alike today and quietly stop doing so the moment
//! someone re-indents the block, which is the same silent failure one edit
//! away. A long line is the price; `rustfmt` does not touch the inside of a
//! string literal, so it stays a long line.
//!
//! ## Why this crate
//!
//! `i18n` is here, so the rule about how translatable strings are spelled is
//! here as well. It does mean the check runs under CTest's `rust-test-eds`
//! rather than a plain `cargo test`, this crate needing the EDS headers; that
//! is the same place every other check on module-facing behaviour runs.

use std::fs;
use std::path::{Path, PathBuf};

/// The call sites `xgettext` is pointed at, as they are spelled in Rust.
///
/// Kept in step with the `--keyword` arguments the `.pot` is generated with:
/// a keyword the extractor knows and this list does not is a string that can
/// go unlisted, which is the whole failure this file exists to prevent.
const MARKERS: [&str; 3] = ["N_(", "translate(", "translate_with("];

/// The root of the checkout, from this crate's manifest directory.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("the crate directory is three levels below the checkout root")
        .to_path_buf()
}

/// The paths listed in `po/POTFILES.in`, in file order.
///
/// `#` comments and blank lines are gettext's own conventions for the file and
/// are not paths.
fn listed(root: &Path) -> Vec<String> {
    let path = root.join("po/POTFILES.in");
    let text = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "po/POTFILES.in is what a message catalogue is built from, and it \
             could not be read ({error}): {}",
            path.display()
        )
    });
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// Every Rust source under `rust/crates/*/src`, as a path relative to `root`.
///
/// `tests/` is deliberately not walked. A test may well write a marker — to
/// check the marker — and a string that exists only in a test binary is not one
/// any user reads.
fn sources(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let crates = root.join("rust/crates");
    let mut dirs: Vec<PathBuf> = fs::read_dir(&crates)
        .expect("rust/crates is where the crates are")
        .map(|entry| {
            entry
                .expect("a readable directory entry")
                .path()
                .join("src")
        })
        .filter(|src| src.is_dir())
        .collect();

    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir).expect("a readable source directory") {
            let path = entry.expect("a readable directory entry").path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let relative = path
                    .strip_prefix(root)
                    .expect("a path under the checkout root");
                found.push(relative.to_string_lossy().into_owned());
            }
        }
    }
    found.sort();
    found
}

/// A source with its comments blanked out, ready to be matched against.
///
/// Comments are not code to `xgettext` either, and a doc comment may spell a
/// marker out — this file's own module docs do. Whole-line only: a marker never
/// shares a line with the `//` that would precede it.
///
/// The lines are *emptied* rather than dropped, so that an index into the
/// result still names the line it came from. A check that reports where a
/// problem is has to be able to count.
fn code_of(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.trim_start().starts_with("//") {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The text of a source `po/POTFILES.in` names.
fn source_text(root: &Path, path: &str) -> String {
    fs::read_to_string(root.join(path)).unwrap_or_else(|error| {
        panic!("po/POTFILES.in lists {path}, which cannot be read ({error})")
    })
}

/// Whether `path` holds a string marked for extraction.
fn marks_a_string(root: &Path, path: &str) -> bool {
    let code = code_of(&source_text(root, path));
    MARKERS
        .iter()
        .any(|marker| calls_on_a_literal(&code, marker))
}

/// Whether `code` hands `marker` a C string literal, wherever the formatter
/// put the line break.
///
/// `marker` ends at the opening parenthesis, and what has to follow — once the
/// whitespace `rustfmt` may have inserted is skipped — is `c"`, the start of a
/// literal. Nothing here tries to match the closing parenthesis or the argument
/// after it: this is a check on whether a file contributes strings at all, not
/// a parser.
fn calls_on_a_literal(code: &str, marker: &str) -> bool {
    code.match_indices(marker)
        .any(|(at, _)| code[at + marker.len()..].trim_start().starts_with("c\""))
}

/// A literal handed to a marker, as its bytes stand in the source.
struct Marked {
    /// The 1-based line the literal opens on.
    line: usize,
    /// The literal's source text, between the opening `c"` and its closing `"`
    /// — escapes as written, not as either language reads them.
    raw: String,
}

/// Every marked literal in `code`, wherever the formatter put it.
fn marked_literals(code: &str) -> Vec<Marked> {
    let mut found = Vec::new();
    for marker in MARKERS {
        for (at, _) in code.match_indices(marker) {
            let after = at + marker.len();
            let opens = after + (code[after..].len() - code[after..].trim_start().len());
            let Some(body) = code[opens..].strip_prefix("c\"") else {
                // A marker on something that is not a literal: a message looked
                // up through a constant, which contributes nothing to extract.
                continue;
            };
            let end = closing_quote(body).unwrap_or_else(|| {
                panic!("a c-string literal that never closes, at byte {opens} — the source does not compile")
            });
            found.push(Marked {
                line: code[..opens].matches('\n').count() + 1,
                raw: body[..end].to_owned(),
            });
        }
    }
    found
}

/// Where the literal starting just past a `c"` ends.
///
/// A backslash consumes what follows it, which is what keeps an escaped quote
/// from being read as the end. Walking bytes is safe for the same reason it is
/// enough: `\` and `"` are ASCII, and no byte of a multi-byte character can be
/// mistaken for either.
fn closing_quote(body: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'"' => return Some(at),
            _ => at += 1,
        }
    }
    None
}

#[test]
fn every_source_that_marks_a_string_is_listed_in_potfiles() {
    let root = repo_root();
    let listed = listed(&root);

    let missing: Vec<String> = sources(&root)
        .into_iter()
        .filter(|path| marks_a_string(&root, path))
        .filter(|path| !listed.contains(path))
        .collect();

    assert!(
        missing.is_empty(),
        "these sources mark strings for translation and are absent from \
         po/POTFILES.in, so the strings would never reach a translator: {missing:?}"
    );
}

#[test]
fn every_path_listed_in_potfiles_still_marks_a_string() {
    let root = repo_root();

    let stale: Vec<String> = listed(&root)
        .into_iter()
        .filter(|path| !marks_a_string(&root, path))
        .collect();

    assert!(
        stale.is_empty(),
        "po/POTFILES.in lists these, and none of them marks a string any more — \
         either the strings moved and their new home is unlisted, or the entry \
         should go: {stale:?}"
    );
}

#[test]
fn every_marked_literal_is_written_on_one_line() {
    let root = repo_root();
    let mut literals = 0;
    let mut spanning = Vec::new();

    for path in listed(&root) {
        for literal in marked_literals(&code_of(&source_text(&root, &path))) {
            literals += 1;
            if literal.raw.contains('\n') {
                spanning.push(format!("{path}:{}", literal.line));
            }
        }
    }

    assert!(
        literals > 0,
        "no marked literal was found in any listed source, which cannot be true \
         while po/POTFILES.in lists any — the scanner in this file has stopped \
         recognising the markers, and every check here is passing vacuously"
    );
    assert!(
        spanning.is_empty(),
        "these marked literals are written across lines, so the msgid xgettext \
         extracts is not the msgid the program looks up and no translation of \
         them can ever be found — put each on a single line: {spanning:?}"
    );
}
