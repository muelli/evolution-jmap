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
//! A literal handed straight to one of the two functions in
//! [`jmap_backend_core::i18n`] that gettext is told to key on — spelled
//! `N_(c"…")` or `translate(c"…")` in the source, which is the form
//! `xgettext --keyword` recognises. Deliberately a textual match on the call
//! site rather than anything cleverer: it is the same thing `xgettext` does, so
//! it agrees with the tool by construction, including on the cases where the
//! tool is the one being crude.
//!
//! Two consequences worth knowing before adding a string:
//!
//! - **A marker whose argument is not a literal is not a marked string** —
//!   `translate(NAME)` looks a message up but contributes nothing to extract,
//!   because the literal is wherever `NAME` was written. That is why the
//!   `c"` is part of the pattern.
//! - **Line comments do not count.** They are stripped before the match, so a
//!   doc comment may spell a marker out — this file's own module docs do —
//!   without putting the file in the list. `xgettext` skips comments too.
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
const MARKERS: [&str; 2] = ["N_(c\"", "translate(c\""];

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

/// Whether `path` holds a string marked for extraction.
fn marks_a_string(root: &Path, path: &str) -> bool {
    let text = fs::read_to_string(root.join(path)).unwrap_or_else(|error| {
        panic!("po/POTFILES.in lists {path}, which cannot be read ({error})")
    });
    text.lines()
        // Comments are not code to `xgettext` either. Whole-line only: a
        // marker never shares a line with the `//` that would precede it.
        .filter(|line| !line.trim_start().starts_with("//"))
        .any(|line| MARKERS.iter().any(|marker| line.contains(marker)))
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
