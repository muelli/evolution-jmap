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

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("the crate directory is three levels below the checkout root")
        .to_path_buf()
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
    let path = repo_root().join("docs/MILESTONES.md");
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
    let path = repo_root().join("docs/MILESTONES.md");
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
    let path = repo_root().join("docs/MILESTONES.md");
    if !path.exists() {
        return;
    }
    let content = fs::read_to_string(&path).expect("read docs/MILESTONES.md");
    let entries = parse_milestones(&content);

    for entry in entries {
        let output = std::process::Command::new("git")
            .current_dir(repo_root())
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
