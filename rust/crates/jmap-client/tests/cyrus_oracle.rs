// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for ci/cyrus-oracle.sh existence, digest pinning, environment export, and tool checks.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

fn script_path() -> PathBuf {
    if let Ok(path) = std::env::var("CYRUS_ORACLE_SCRIPT") {
        let p = PathBuf::from(path);
        if p.exists() {
            return p;
        }
    }
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let relative = PathBuf::from(manifest_dir).join("../../../ci/cyrus-oracle.sh");
    if relative.exists() {
        return relative.canonicalize().unwrap_or(relative);
    }
    if let Ok(pwd) = std::env::var("PWD") {
        let mut cur = PathBuf::from(pwd);
        loop {
            let candidate = cur.join("ci/cyrus-oracle.sh");
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
            let candidate = cur.join("ci/cyrus-oracle.sh");
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
            "cyrus-oracle-scratch-bin-{}-{}",
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
fn cyrus_oracle_script_exists_and_pins_image_by_digest() {
    let path = script_path();
    assert!(path.exists(), "ci/cyrus-oracle.sh must exist at {:?}", path);
    assert!(path.is_file(), "ci/cyrus-oracle.sh must be a file");

    let metadata = fs::metadata(&path).expect("metadata of ci/cyrus-oracle.sh");
    let mode = metadata.permissions().mode();
    assert_ne!(
        mode & 0o111,
        0,
        "ci/cyrus-oracle.sh must have executable bits set (mode: {:o})",
        mode
    );

    let content = fs::read_to_string(&path).expect("read ci/cyrus-oracle.sh");
    assert!(
        content.starts_with("#!/usr/bin/env bash"),
        "ci/cyrus-oracle.sh must start with #!/usr/bin/env bash"
    );
    let spdx_needle = concat!("SPDX-License-", "Identifier: GPL-3.0-or-later");
    assert!(
        content.contains(spdx_needle),
        "ci/cyrus-oracle.sh must include SPDX license header"
    );
    assert!(
        !content.contains('\u{2014}'),
        "ci/cyrus-oracle.sh must not contain em-dashes"
    );
    assert!(
        content.contains(
            "ghcr.io/cyrusimap/cyrus-docker-test-server@sha256:035aa4ffdb84df95a1057645d05c9a156dde74e5787ef1a11f7d9c244a38b2c0"
        ),
        "ci/cyrus-oracle.sh must pin cyrus-docker-test-server by sha256 digest"
    );
}

#[test]
fn cyrus_oracle_env_subcommand_and_sourcing_export_live_server_vars() {
    let path = script_path();
    assert!(path.exists(), "ci/cyrus-oracle.sh must exist");

    let bash_bin = find_host_tool("bash").unwrap_or_else(|| PathBuf::from("bash"));
    let output = Command::new(&bash_bin)
        .arg(&path)
        .arg("env")
        .env("CYRUS_ORACLE_HOST", "127.0.0.1")
        .env("CYRUS_ORACLE_HTTP_PORT", "18080")
        .env("CYRUS_ORACLE_WEB_PORT", "18001")
        .output()
        .expect("execute ci/cyrus-oracle.sh env");

    assert!(
        output.status.success(),
        "ci/cyrus-oracle.sh env must succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("export JMAP_LIVE_SERVER_URL=\"http://127.0.0.1:18080\""),
        "env output must export JMAP_LIVE_SERVER_URL, got: {}",
        stdout
    );
    assert!(
        stdout.contains("export JMAP_LIVE_SERVER_USER="),
        "env output must export JMAP_LIVE_SERVER_USER, got: {}",
        stdout
    );
    assert!(
        stdout.contains("export JMAP_LIVE_SERVER_WRITE_USER="),
        "env output must export JMAP_LIVE_SERVER_WRITE_USER, got: {}",
        stdout
    );
    assert!(
        stdout.contains("export JMAP_LIVE_SERVER_RECIPIENT_USER="),
        "env output must export JMAP_LIVE_SERVER_RECIPIENT_USER, got: {}",
        stdout
    );

    let sourced = Command::new(&bash_bin)
        .arg("-c")
        .arg("source \"$1\" env >/dev/null && printf '%s|%s|%s' \"$JMAP_LIVE_SERVER_URL\" \"$JMAP_LIVE_SERVER_WRITE_USER\" \"$JMAP_LIVE_SERVER_RECIPIENT_USER\"")
        .arg("bash")
        .arg(&path)
        .env("CYRUS_ORACLE_HOST", "127.0.0.1")
        .env("CYRUS_ORACLE_HTTP_PORT", "19080")
        .output()
        .expect("source ci/cyrus-oracle.sh env");

    assert!(
        sourced.status.success(),
        "sourcing ci/cyrus-oracle.sh must succeed, stderr: {}",
        String::from_utf8_lossy(&sourced.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&sourced.stdout),
        "http://127.0.0.1:19080|writer|recipient"
    );
}

#[test]
fn cyrus_oracle_fails_loudly_when_podman_missing_on_start() {
    let path = script_path();
    assert!(path.exists(), "ci/cyrus-oracle.sh must exist");

    let scratch = ScratchBinDir::new();
    scratch.provide_tool("bash");
    scratch.provide_tool("curl");

    let bash_bin = find_host_tool("bash").unwrap_or_else(|| PathBuf::from("bash"));
    let output = Command::new(&bash_bin)
        .arg(&path)
        .arg("start")
        .env("PATH", scratch.path())
        .output()
        .expect("execute ci/cyrus-oracle.sh start without podman");

    assert!(
        !output.status.success(),
        "ci/cyrus-oracle.sh start must fail when podman is missing"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing required tool: podman"),
        "stderr must report missing required tool: podman, got: {}",
        stderr
    );
}

#[test]
fn cyrus_oracle_rejects_unknown_subcommand() {
    let path = script_path();
    assert!(path.exists(), "ci/cyrus-oracle.sh must exist");

    let bash_bin = find_host_tool("bash").unwrap_or_else(|| PathBuf::from("bash"));
    let output = Command::new(&bash_bin)
        .arg(&path)
        .arg("unknown-subcommand")
        .output()
        .expect("execute ci/cyrus-oracle.sh unknown-subcommand");

    assert!(
        !output.status.success(),
        "ci/cyrus-oracle.sh must fail on an unknown subcommand"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Usage:"),
        "stderr must print usage on unknown subcommand, got: {}",
        stderr
    );
}
