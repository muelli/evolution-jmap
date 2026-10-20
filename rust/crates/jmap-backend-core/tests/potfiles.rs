// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `po/POTFILES.in` against the sources it claims to list.
//!
//! `xgettext` builds a message catalogue from the files named in
//! `POTFILES.in` and from no other files. Thus a source that marks a string
//! and is not in that list causes no build error, no warning, and nothing
//! visible. The string is absent from the `.pot`, no translator gets it, and
//! it ships in English forever. The standing directive asks for a check that
//! catches this failure. This file is that check.
//!
//! It checks the list in both directions, because both failures are silent:
//!
//! - **Nothing marked is missing from the list.** The failure above.
//! - **Nothing in the list is stale.** A path that no longer exists makes
//!   `xgettext` fail, which is at least visible. A path that exists but no
//!   longer marks a string is worse. It continues to work while its old
//!   strings silently leave the catalogue.
//!
//! ## What counts as a marked string
//!
//! A literal given directly to one of the functions in
//! [`jmap_backend_core::i18n`] that gettext uses as keywords. In the source it
//! is `N_(c"…")`, `translate(c"…")` or `translate_with(c"…", …)`, which is the
//! form that `xgettext --keyword` recognises. The match is intentionally
//! textual on the call site, not more clever. It is similar to what `xgettext`
//! does, so it agrees with the tool where the tool is crude.
//!
//! Three results to know before you add a string:
//!
//! - **A marker whose argument is not a literal is not a marked string.**
//!   `translate(NAME)` looks up a message but gives nothing to extract,
//!   because the literal is where `NAME` is written. Thus the `c"` is part of
//!   the pattern.
//! - **Line comments do not count.** The check removes them before the match.
//!   Thus a doc comment can contain a marker (this file's module docs do)
//!   without putting the file in the list. `xgettext` also skips comments.
//! - **The literal can be on a line after the marker.** `rustfmt` moves it to
//!   the next line when the call is too wide. A message long enough to need
//!   translation causes this. `xgettext` lexes tokens, not lines, so this has
//!   no effect on it. A match line by line here would silently miss the longest
//!   strings. Thus the check skips the whitespace between the two, as the lexer
//!   does.
//!
//! ## And a marked literal must be written on one line
//!
//! A correct list is necessary but not sufficient. `xgettext` must also read
//! the literal as `rustc` does. If not, the msgid in the catalogue is not the
//! msgid that the program looks up, and the program never finds the
//! translation. The two lex the same bytes, and they differ on only one
//! construct that this project can write: a backslash at the end of a line.
//! Rust drops the newline *and* the indentation after it. C's line splicing
//! drops only the newline. So
//!
//! ```text
//! c"This event repeats until %1$s, and the time zone it is in, \
//!   %2$s, …"
//! ```
//!
//! has one space before `%2$s` in the lookup and fifteen in the extraction.
//! The failure is fully silent: the extraction succeeds, the catalogue looks
//! correct, the translator translates it, and the user still reads English.
//!
//! Thus this check enforces the rule "no marked literal spans lines", not the
//! narrower rule "no *indentation* after the continuation". A continuation at
//! column zero reads the same in both languages today. But it silently stops
//! when someone re-indents the block, so the same silent failure is one edit
//! away. The cost is a long line. `rustfmt` does not change the inside of a
//! string literal, so the line stays long.
//!
//! ## And the catalogue in the tree must agree with the sources
//!
//! The two rules above predict what `xgettext -L C` does. This last rule reads
//! what it *did*. `po/evolution-jmap.pot` is the committed extraction output.
//! The check compares its messages with the marked literals in both
//! directions.
//!
//! This is important because `-L C` is crude in ways that nobody has listed.
//! One measured example: gettext's C lexer reads `'` as the start of a
//! character constant and stops at the end of the line. Thus it discards a
//! marked string on a line with an *odd* number of apostrophes (for example,
//! one lifetime). The catalogue loses the string, and the only sign is a
//! `warning: unterminated character constant` that names the line. And
//!
//! ```text
//! const NAME: &'static CStr = N_(c"JMAP");
//! ```
//!
//! is a normal line to write. The rule cannot be "no lifetime near a string".
//! That is only one instance of a class whose other members are unknown. A
//! comparison with the real output of the tool catches the full class,
//! including this instance.
//!
//! Both directions, because both failures are silent. A message that the
//! sources mark and the catalogue does not have was discarded or is stale, and
//! ships in English. A message that the catalogue has and no source marks is
//! work for translators on text that no user sees. It also hides a *changed*
//! string, because the edit shows as one loss and one orphan, not as nothing.
//!
//! Not checked: the `#:` source references. They move when any line above
//! them moves. They are hints for a translator, and no lookup uses them. Exact
//! references would make every unrelated edit in a listed file fail, with no
//! benefit.
//!
//! ## Why this crate
//!
//! `i18n` is in this crate, so the rule about the spelling of translatable
//! strings is also here. Because this crate needs the EDS headers, the check
//! runs under CTest's `rust-test-eds`, not a plain `cargo test`. Every other
//! check on module-facing behaviour runs there too.

use std::fs;
use std::path::{Path, PathBuf};

/// The call sites that `xgettext` extracts, as they are spelled in Rust.
///
/// Keep this list the same as the `--keyword` arguments that generate the
/// `.pot`. If the extractor knows a keyword that this list does not, a string
/// can be absent from the list. This file exists to prevent that failure.
const MARKERS: [&str; 3] = ["N_(", "translate(", "translate_with("];

/// Walks upward from `start` to find the checkout root (marked by `.git`).
/// Returns `None` if no ancestor has one. An example is a mutation-testing
/// sandbox that copies only the `rust/` subtree. There, `po/` is not copied,
/// so the test cannot make a valid assertion.
fn find_repo_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

/// The root of the checkout, from this crate's manifest directory.
fn repo_root() -> Option<PathBuf> {
    find_repo_root(Path::new(env!("CARGO_MANIFEST_DIR")))
}

/// The paths listed in `po/POTFILES.in`, in file order.
///
/// `#` comments and blank lines are gettext's conventions for the file. They
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
/// This intentionally does not walk `tests/`. A test can write a marker to
/// check the marker. No user reads a string that is only in a test binary.
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

/// A source with its comment lines made empty, for the match.
///
/// `xgettext` also does not read comments as code. A doc comment can contain a
/// marker (this file's module docs do). Only full lines are removed: a marker
/// is never on the same line after a `//`.
///
/// The lines are *emptied*, not dropped, so a line index into the result is
/// the same as in the source. A check that reports the location of a problem
/// must count lines correctly.
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
/// `marker` ends at the opening parenthesis. After any whitespace that
/// `rustfmt` inserted, `c"` (the start of a literal) must follow. This does not
/// match the closing parenthesis or the argument after it. It checks only if a
/// file contributes strings. It is not a parser.
fn calls_on_a_literal(code: &str, marker: &str) -> bool {
    code.match_indices(marker)
        .any(|(at, _)| code[at + marker.len()..].trim_start().starts_with("c\""))
}

/// A literal handed to a marker, as its bytes stand in the source.
struct Marked {
    /// The 1-based line the literal opens on.
    line: usize,
    /// The literal's source text, between the opening `c"` and its closing
    /// `"`. Escapes are as written, not as either language reads them.
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
                // A marker on a value that is not a literal: a message that
                // the code looks up through a constant. It gives nothing to
                // extract.
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
/// A backslash consumes the next byte, so an escaped quote is not read as the
/// end. A walk over bytes is safe and sufficient: `\` and `"` are ASCII, and
/// no byte of a multi-byte character can look like either.
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

/// The catalogue `po/extract.sh` writes, as a path under the checkout root.
const CATALOGUE: &str = "po/evolution-jmap.pot";

/// Every message in the extracted catalogue, in file order.
///
/// This drops the header entry (the entry with an empty msgid, whose msgstr
/// has `Content-Type` and the other fields), because it is metadata, not a
/// message. It also drops obsolete entries, which gettext writes as comments
/// (`#~ msgid`). They record what a message *was*, and no lookup can get them.
fn catalogue_messages(root: &Path) -> Vec<String> {
    let path = root.join(CATALOGUE);
    let text = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{CATALOGUE} is the catalogue this project's translators work from, \
             and it could not be read ({error}) — run po/extract.sh to write it"
        )
    });

    let lines: Vec<&str> = text.lines().collect();
    let mut messages = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        let Some(first) = lines[at].strip_prefix("msgid ") else {
            at += 1;
            continue;
        };
        // gettext writes a wrapped message as an empty first part and then
        // continuation lines. Thus the parts are joined, not read separately.
        let mut message = quoted(first.trim());
        at += 1;
        while at < lines.len() && lines[at].starts_with('"') {
            message.push_str(&quoted(lines[at].trim()));
            at += 1;
        }
        if !message.is_empty() {
            messages.push(message);
        }
    }
    messages
}

/// The text of one `"…"` part of a catalogue entry, escapes resolved.
fn quoted(line: &str) -> String {
    let body = line
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| panic!("a catalogue line that is not a quoted string: {line}"));
    unescape(body, "the catalogue")
}

/// A literal's bytes as the language that owns them reads it.
///
/// Both sides of this comparison spell the same small set of escapes the same
/// way, so one decoder is sufficient for the source and for the catalogue. An
/// escape outside that set causes a `panic!`, not a guess. The two languages
/// agree on the escapes written here today. The first construct where they
/// can disagree (for example, Rust's `\u{2014}` against the literal U+2014
/// character in C) must stop this check. It must not be silently read as one
/// of them.
fn unescape(raw: &str, whose: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some(other) => panic!(
                "\\{other} in {whose} is an escape this check does not model, and \
                 Rust and C need not read it alike — teach `unescape` what it \
                 means in both before writing it: {raw}"
            ),
            None => panic!("a trailing backslash in {whose}: {raw}"),
        }
    }
    out
}

/// Every marked literal in every listed source, as the program reads it.
fn marked_messages(root: &Path) -> Vec<String> {
    listed(root)
        .iter()
        .flat_map(|path| {
            marked_literals(&code_of(&source_text(root, path)))
                .into_iter()
                .map(|literal| unescape(&literal.raw, "a source"))
        })
        .collect()
}

#[test]
fn every_source_that_marks_a_string_is_listed_in_potfiles() {
    let Some(root) = repo_root() else {
        eprintln!(
            "skipping every_source_that_marks_a_string_is_listed_in_potfiles: no \
             `.git` ancestor found, not running inside a full repository checkout"
        );
        return;
    };
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
    let Some(root) = repo_root() else {
        return;
    };

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
    let Some(root) = repo_root() else {
        return;
    };
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

#[test]
fn every_marked_literal_reached_the_catalogue() {
    let Some(root) = repo_root() else {
        return;
    };
    let extracted = catalogue_messages(&root);
    let marked = marked_messages(&root);

    assert!(
        !marked.is_empty(),
        "no marked literal was found in any listed source, so this check is \
         passing over an empty set — the scanner has stopped recognising the \
         markers"
    );

    let lost: Vec<&String> = marked
        .iter()
        .filter(|message| !extracted.contains(message))
        .collect();

    assert!(
        lost.is_empty(),
        "these strings are marked for translation and are not in {CATALOGUE}, \
         so no translator is offered them and they ship in English — either the \
         catalogue is stale (run po/extract.sh) or xgettext did not see them \
         where they stand: {lost:?}"
    );
}

#[test]
fn every_message_in_the_catalogue_is_still_marked_in_a_source() {
    let Some(root) = repo_root() else {
        return;
    };
    let marked = marked_messages(&root);

    let orphaned: Vec<String> = catalogue_messages(&root)
        .into_iter()
        .filter(|message| !marked.contains(message))
        .collect();

    assert!(
        orphaned.is_empty(),
        "{CATALOGUE} holds these messages and no source marks them any more, so \
         translators are working on text nobody will ever read — run \
         po/extract.sh: {orphaned:?}"
    );
}

/// A scratch directory under the system temp dir. It is never inside this
/// checkout, so no real `.git` ancestor can affect the assertions below. It is
/// unique per call, so parallel tests do not collide.
fn scratch_dir(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "jmap-backend-core-potfiles-test-{name}-{}-{n}",
        std::process::id()
    ))
}

#[test]
fn find_repo_root_finds_the_nearest_git_ancestor() {
    let base = scratch_dir("hit");
    let nested = base.join("a/b/c");
    fs::create_dir_all(&nested).expect("create nested scratch dirs");
    fs::create_dir_all(base.join(".git")).expect("create fake .git marker");

    assert_eq!(find_repo_root(&nested), Some(base.clone()));

    fs::remove_dir_all(&base).ok();
}

#[test]
fn find_repo_root_returns_none_without_a_git_ancestor() {
    let base = scratch_dir("miss");
    let nested = base.join("x/y");
    fs::create_dir_all(&nested).expect("create nested scratch dirs");
    // Intentionally no `.git` under `base`. The system temp dir and its
    // ancestors are not part of a git checkout.

    assert_eq!(find_repo_root(&nested), None);

    fs::remove_dir_all(&base).ok();
}
