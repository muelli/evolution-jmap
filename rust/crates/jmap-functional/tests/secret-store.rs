// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests the environment of the functional harness, not a backend.
//!
//! The private session bus of the gated functional CI job has no secret
//! service. Thus `evolution_source_registry_creates_and_deletes_a_calendar`
//! (and each account with an `[Authentication]` extension) fails when
//! `stored_password_of` in `create_resource_sync` calls
//! `e_source_credentials_provider_lookup_sync`. `Session::run` unlocks a login
//! keyring on the private bus before the real client runs. Its doc comment
//! tells why this step, and not only an installed `gnome-keyring`, prevents
//! the two failure modes. This test proves that the step runs and creates the
//! on-disk keyring file, with no backend, module or `ESource`.
//!
//! The test does not need a built module, a client library or a
//! `JMAP_FUNCTIONAL_*` path. It needs only `dbus-run-session` and
//! `gnome-keyring-daemon`. Thus it also runs under a plain
//! `cargo test -p jmap-functional --test secret-store`. It is a
//! `functional`-labelled ctest (see `cmake/Functional.cmake`), so the
//! `-DENABLE_FUNCTIONAL_TESTS=ON` build also finds a missing `gnome-keyring`.

use jmap_functional::Session;
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn a_session_creates_a_usable_login_keyring_before_running_its_client() {
    let session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/secret-store"));

    // `/bin/true` replaces the real client. This test examines the step that
    // `run()` does before the client starts.
    let output = session.run(&PathBuf::from("/bin/true"), &[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the trivial client failed with {}\n--- stderr ---\n{stderr}",
        output.status
    );

    let keyring = session.login_keyring_file();
    assert!(
        keyring.exists(),
        "no login keyring at {}: gnome-keyring-daemon's --unlock either was \
         not run or did not create one\n--- stderr ---\n{stderr}",
        keyring.display()
    );
}

#[test]
fn running_a_client_does_not_leave_a_keyring_daemon_behind() {
    // `--daemonize` detaches `gnome-keyring-daemon` from the private bus. It
    // stays alive after `dbus-run-session` if `run` does not stop it.
    let session = Session::new(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/secret-store-no-leak"
    ));

    let output = session.run(&PathBuf::from("/bin/true"), &[]);
    assert!(output.status.success());

    assert!(
        keyring_daemons(&session.runtime_directory()).is_empty(),
        "a gnome-keyring-daemon for this session's own XDG_RUNTIME_DIR ({}) is \
         still alive after Session::run returned",
        session.runtime_directory().display()
    );
}

/// Returns each `gnome-keyring-daemon` process with `XDG_RUNTIME_DIR` equal
/// to `runtime_directory`. `jmap-backend-core/tests/secret_store.rs` uses the
/// same match.
fn keyring_daemons(runtime_directory: &Path) -> Vec<i32> {
    let marker = format!("XDG_RUNTIME_DIR={}", runtime_directory.display());
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<i32>().ok()?;
            let cmdline = fs::read(entry.path().join("cmdline")).ok()?;
            let environ = fs::read(entry.path().join("environ")).ok()?;
            (contains(&cmdline, b"gnome-keyring-daemon")
                && environ
                    .split(|byte| *byte == 0)
                    .any(|value| value == marker.as_bytes()))
            .then_some(pid)
        })
        .collect()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
