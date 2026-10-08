// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for ci/consumer-check.sh script existence, permissions, and tool validations.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

fn script_path() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .join("../../../ci/consumer-check.sh")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(manifest_dir).join("../../../ci/consumer-check.sh"))
}

#[test]
fn consumer_check_script_exists_and_is_executable() {
    let path = script_path();
    assert!(
        path.exists(),
        "ci/consumer-check.sh must exist at {:?}",
        path
    );
    assert!(path.is_file(), "ci/consumer-check.sh must be a file");

    let metadata = fs::metadata(&path).expect("metadata of ci/consumer-check.sh");
    let permissions = metadata.permissions();
    let mode = permissions.mode();
    assert_ne!(
        mode & 0o111,
        0,
        "ci/consumer-check.sh must have executable bits set (mode: {:o})",
        mode
    );

    let content = fs::read_to_string(&path).expect("read ci/consumer-check.sh");
    assert!(
        content.starts_with("#!/usr/bin/env bash"),
        "ci/consumer-check.sh must start with #!/usr/bin/env bash shebang"
    );
    assert!(
        content.contains("set -euo pipefail"),
        "ci/consumer-check.sh must use strict bash error handling (set -euo pipefail)"
    );
    let spdx_needle = concat!("SPDX-License-", "Identifier: GPL-3.0-or-later");
    assert!(
        content.contains(spdx_needle),
        "ci/consumer-check.sh must include SPDX license header"
    );
    assert!(
        !content.contains('\u{2014}'),
        "ci/consumer-check.sh must not contain em-dashes"
    );
}

#[test]
fn consumer_check_script_fails_loudly_when_tools_missing() {
    let path = script_path();
    if !path.exists() {
        panic!("ci/consumer-check.sh does not exist yet");
    }

    // Provide bash in PATH (or invoke bash directly), but exclude cargo
    let bash_dir = PathBuf::from("/usr/bin");
    let output = Command::new("bash")
        .arg(&path)
        .env("PATH", bash_dir)
        .output()
        .expect("execution of ci/consumer-check.sh without cargo in PATH");

    assert!(
        !output.status.success(),
        "ci/consumer-check.sh must fail when required tools are missing"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing required tool: cargo"),
        "stderr must loudly announce missing required tool: cargo, got: {}",
        stderr
    );
}

#[test]
fn consumer_check_script_clean_of_agent_bookkeeping_citations() {
    let path = script_path();
    if !path.exists() {
        panic!("ci/consumer-check.sh does not exist yet");
    }

    let content = fs::read_to_string(&path).expect("read ci/consumer-check.sh");
    let forbidden = [
        "NIGHT-LOG",
        "AGY-LOG",
        "AGY-TASKS",
        "BACKLOG",
        "MILESTONES",
        "ROADMAP",
    ];

    for term in forbidden {
        assert!(
            !content.contains(term),
            "ci/consumer-check.sh must not cite {} (boundary lint compliance)",
            term
        );
    }
}
