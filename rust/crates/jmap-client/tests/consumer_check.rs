// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for ci/consumer-check.sh script existence, permissions, and tool validations.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

fn script_path() -> PathBuf {
    if let Ok(path) = std::env::var("CONSUMER_CHECK_SCRIPT") {
        let p = PathBuf::from(path);
        if p.exists() {
            return p;
        }
    }
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let relative = PathBuf::from(manifest_dir).join("../../../ci/consumer-check.sh");
    if relative.exists() {
        return relative.canonicalize().unwrap_or(relative);
    }
    if let Ok(pwd) = std::env::var("PWD") {
        let mut cur = PathBuf::from(pwd);
        loop {
            let candidate = cur.join("ci/consumer-check.sh");
            if candidate.exists() {
                return candidate.canonicalize().unwrap_or(candidate);
            }
            if !cur.pop() {
                break;
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let mut cur = cwd.as_path();
        loop {
            let candidate = cur.join("ci/consumer-check.sh");
            if candidate.exists() {
                return candidate.canonicalize().unwrap_or(candidate);
            }
            match cur.parent() {
                Some(p) => cur = p,
                None => break,
            }
        }
    }
    relative
}

fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
        && path
            .metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

fn find_host_tool(name: &str) -> Option<PathBuf> {
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(name);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    for dir in &["/usr/bin", "/bin", "/usr/local/bin"] {
        let candidate = PathBuf::from(dir).join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

struct ScratchBinDir {
    path: PathBuf,
}

impl ScratchBinDir {
    fn new() -> Self {
        let unique = format!(
            "consumer-check-scratch-bin-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        fs::create_dir_all(&path).expect("create scratch bin dir");
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }

    fn link_host_tool(&self, name: &str) -> bool {
        if let Some(target) = find_host_tool(name) {
            let link = self.path.join(name);
            let _ = fs::remove_file(&link);
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &link).expect("symlink tool into scratch dir");
            true
        } else {
            false
        }
    }

    fn provide_tool(&self, name: &str) {
        if !self.link_host_tool(name) {
            let tool_path = self.path.join(name);
            fs::write(&tool_path, "#!/bin/sh\nexit 0\n").expect("write dummy tool");
            let mut perms = fs::metadata(&tool_path).expect("metadata").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&tool_path, perms).expect("set executable permissions");
        }
    }
}

impl Drop for ScratchBinDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
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

    // Build a scratch PATH that demonstrably lacks cargo (empty temp bin dir plus symlinks
    // to just what ci/consumer-check.sh needs) instead of assuming where cargo lives.
    let scratch = ScratchBinDir::new();
    scratch.provide_tool("bash");
    scratch.provide_tool("tar");
    scratch.provide_tool("mktemp");
    scratch.provide_tool("sed");

    let bash_bin = find_host_tool("bash").unwrap_or_else(|| PathBuf::from("bash"));
    let output = Command::new(&bash_bin)
        .arg(&path)
        .env("PATH", scratch.path())
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
fn consumer_check_script_fails_loudly_when_each_tool_missing() {
    let path = script_path();
    if !path.exists() {
        panic!("ci/consumer-check.sh does not exist yet");
    }

    let bash_bin = find_host_tool("bash").unwrap_or_else(|| PathBuf::from("bash"));

    for missing in ["cargo", "tar", "mktemp", "sed"] {
        let scratch = ScratchBinDir::new();
        scratch.provide_tool("bash");
        for tool in ["cargo", "tar", "mktemp", "sed"] {
            if tool != missing {
                scratch.provide_tool(tool);
            }
        }

        let output = Command::new(&bash_bin)
            .arg(&path)
            .env("PATH", scratch.path())
            .output()
            .expect("execution of ci/consumer-check.sh with missing tool");

        assert!(
            !output.status.success(),
            "ci/consumer-check.sh must fail when {} is missing",
            missing
        );

        let stderr = String::from_utf8_lossy(&output.stderr);
        let expected = format!("missing required tool: {}", missing);
        assert!(
            stderr.contains(&expected),
            "stderr must loudly announce {}, got: {}",
            expected,
            stderr
        );
    }
}

#[test]
fn scratch_path_demonstrably_lacks_cargo_regardless_of_host_cargo() {
    let scratch = ScratchBinDir::new();
    scratch.provide_tool("bash");
    scratch.provide_tool("tar");
    scratch.provide_tool("mktemp");
    scratch.provide_tool("sed");

    assert!(
        !scratch.path().join("cargo").exists(),
        "scratch directory must not contain cargo"
    );

    let bash_bin = find_host_tool("bash").unwrap_or_else(|| PathBuf::from("bash"));
    let output = Command::new(&bash_bin)
        .arg("-c")
        .arg("command -v cargo || true")
        .env("PATH", scratch.path())
        .output()
        .expect("check command -v cargo in scratch PATH");

    assert!(
        output.stdout.is_empty(),
        "command -v cargo in scratch PATH must find nothing, found: {}",
        String::from_utf8_lossy(&output.stdout)
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
