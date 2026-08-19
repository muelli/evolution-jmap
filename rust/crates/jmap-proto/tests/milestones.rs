// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for `docs/MILESTONES.md` format and completion tags.
//!
//! `docs/ROADMAP.md` defines the milestone completion protocol:
//! "When a milestone's acceptance criteria are fully met (or a standing directive
//! is fully carried out), append one line to docs/MILESTONES.md and commit it
//! with the work: `<TAG> COMPLETE <short-sha> <ISO-date>` — e.g.
//! `M5 COMPLETE a1b2c3d 2026-08-10`, or `CALCARD COMPLETE …` for the calcard
//! directive. This file is a machine-readable trigger (the re-audit watcher
//! watches it); write a tag only when you would defend the milestone as
//! genuinely done, and never remove or edit prior lines."
//!
//! This test suite ensures that `docs/MILESTONES.md` exists, is formatted
//! strictly according to `<TAG> COMPLETE <short-sha> <ISO-date>`, contains the
//! completed milestones M1-M6 and M8, and enforces valid SHA and date syntax.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Walks upward from `start` looking for the checkout root (marked by
/// `.git`). Returns `None` if no ancestor has one — e.g. a mutation-testing
/// sandbox that copies only the `rust/` subtree, so `docs/` was never copied
/// at all and there is nothing honest to assert here.
fn find_repo_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

fn repo_root() -> Option<PathBuf> {
    find_repo_root(Path::new(env!("CARGO_MANIFEST_DIR")))
}

#[derive(Debug, PartialEq, Eq)]
pub struct MilestoneEntry {
    pub tag: String,
    pub sha: String,
    pub date: String,
}

pub fn parse_milestones(content: &str) -> Vec<MilestoneEntry> {
    let mut entries = Vec::new();
    let mut seen_tags = HashSet::new();

    for (idx, raw_line) in content.lines().enumerate() {
        let line_num = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(
            parts.len(),
            4,
            "line {line_num} must have 4 tokens (<TAG> COMPLETE <short-sha> <ISO-date>), got: '{line}'"
        );

        let tag = parts[0];
        let keyword = parts[1];
        let sha = parts[2];
        let date = parts[3];

        assert_eq!(
            keyword, "COMPLETE",
            "line {line_num} keyword must be COMPLETE, got: '{keyword}'"
        );

        assert!(
            tag.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "line {line_num} tag '{tag}' contains invalid characters"
        );

        assert!(
            sha.len() >= 7 && sha.len() <= 40 && sha.chars().all(|c| c.is_ascii_hexdigit()),
            "line {line_num} sha '{sha}' must be a valid 7-40 char hex commit SHA"
        );

        let date_parts: Vec<&str> = date.split('-').collect();
        assert_eq!(
            date_parts.len(),
            3,
            "line {line_num} date '{date}' must be formatted YYYY-MM-DD"
        );
        let year: u32 = date_parts[0]
            .parse()
            .unwrap_or_else(|_| panic!("line {line_num} invalid year in '{date}'"));
        let month: u32 = date_parts[1]
            .parse()
            .unwrap_or_else(|_| panic!("line {line_num} invalid month in '{date}'"));
        let day: u32 = date_parts[2]
            .parse()
            .unwrap_or_else(|_| panic!("line {line_num} invalid day in '{date}'"));

        assert!(
            year >= 2026 && (1..=12).contains(&month) && (1..=31).contains(&day),
            "line {line_num} date '{date}' is out of range"
        );

        assert!(
            seen_tags.insert(tag.to_string()),
            "line {line_num} duplicate milestone tag '{tag}'"
        );

        entries.push(MilestoneEntry {
            tag: tag.to_string(),
            sha: sha.to_string(),
            date: date.to_string(),
        });
    }

    entries
}

#[test]
fn milestones_file_exists_and_parses() {
    let Some(root) = repo_root() else {
        eprintln!(
            "skipping milestones_file_exists_and_parses: no `.git` ancestor found, \
             not running inside a full repository checkout"
        );
        return;
    };
    let path = root.join("docs/MILESTONES.md");
    assert!(
        path.exists(),
        "docs/MILESTONES.md must exist in the repository root"
    );

    let content = fs::read_to_string(&path).expect("read docs/MILESTONES.md");
    let entries = parse_milestones(&content);

    assert!(
        !entries.is_empty(),
        "docs/MILESTONES.md must contain completion entries"
    );

    let tags: Vec<&str> = entries.iter().map(|e| e.tag.as_str()).collect();
    for expected in ["M1", "M2", "M3", "M4", "M5", "M6", "M8"] {
        assert!(
            tags.contains(&expected),
            "docs/MILESTONES.md is missing expected completed milestone {expected}"
        );
    }
}

#[test]
fn milestones_entries_are_chronological() {
    let Some(root) = repo_root() else {
        return;
    };
    let path = root.join("docs/MILESTONES.md");
    if !path.exists() {
        return;
    }
    let content = fs::read_to_string(&path).expect("read docs/MILESTONES.md");
    let entries = parse_milestones(&content);

    for window in entries.windows(2) {
        assert!(
            window[0].date <= window[1].date,
            "milestone entries must be chronologically ordered: {} ({}) came before {} ({})",
            window[0].tag,
            window[0].date,
            window[1].tag,
            window[1].date
        );
    }
}

#[test]
fn parser_rejects_malformed_entries() {
    // Bad token count
    let res = std::panic::catch_unwind(|| parse_milestones("M1 COMPLETE"));
    assert!(res.is_err());

    // Bad keyword
    let res = std::panic::catch_unwind(|| parse_milestones("M1 DONE 0ac316b 2026-08-08"));
    assert!(res.is_err());

    // Bad SHA (not hex)
    let res = std::panic::catch_unwind(|| parse_milestones("M1 COMPLETE not_a_sha 2026-08-08"));
    assert!(res.is_err());

    // Bad SHA (too short)
    let res = std::panic::catch_unwind(|| parse_milestones("M1 COMPLETE 0a 2026-08-08"));
    assert!(res.is_err());

    // Bad date format
    let res = std::panic::catch_unwind(|| parse_milestones("M1 COMPLETE 0ac316b 08/08/2026"));
    assert!(res.is_err());

    // Duplicate tag
    let res = std::panic::catch_unwind(|| {
        parse_milestones("M1 COMPLETE 0ac316b 2026-08-08\nM1 COMPLETE 3d13b38 2026-08-08")
    });
    assert!(res.is_err());
}

#[test]
fn milestone_commits_exist_in_git_history() {
    let Some(root) = repo_root() else {
        return;
    };
    let path = root.join("docs/MILESTONES.md");
    if !path.exists() {
        return;
    }
    let content = fs::read_to_string(&path).expect("read docs/MILESTONES.md");
    let entries = parse_milestones(&content);

    for entry in entries {
        let output = std::process::Command::new("git")
            .current_dir(&root)
            .args([
                "rev-parse",
                "--verify",
                &format!("{}^{{commit}}", entry.sha),
            ])
            .output()
            .expect("execute git rev-parse");

        assert!(
            output.status.success(),
            "milestone {} commit sha {} not found in git history",
            entry.tag,
            entry.sha
        );
    }
}

/// A scratch directory under the system temp dir, guaranteed not to sit
/// inside this checkout (so no real `.git` ancestor can leak into the
/// assertions below), unique per call so parallel tests don't collide.
fn scratch_dir(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "jmap-proto-milestones-test-{name}-{}-{n}",
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
    // Deliberately no `.git` anywhere under `base`; the system temp dir and
    // its ancestors are not part of a git checkout.

    assert_eq!(find_repo_root(&nested), None);

    fs::remove_dir_all(&base).ok();
}
