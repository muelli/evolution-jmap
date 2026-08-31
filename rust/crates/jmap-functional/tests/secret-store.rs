// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! M9 layer 1's own environment, not a backend under test.
//!
//! The gated functional CI job's private session bus has no secret service,
//! so `evolution_source_registry_creates_and_deletes_a_calendar` (any
//! account with an `[Authentication]` extension, really) fails when
//! `create_resource_sync`'s `stored_password_of` asks
//! `e_source_credentials_provider_lookup_sync` for one. `Session::run` now
//! unlocks a login keyring on the private bus before the real client runs —
//! see its own doc comment for why that specific step, not merely installing
//! `gnome-keyring`, is what closes both failure modes. This test proves that
//! step actually runs and does what it says, with no backend, module, or
//! `ESource` involved: the on-disk keyring file it should have created.
//!
//! It needs nothing `ENABLE_FUNCTIONAL_TESTS`'s other tests do — no built
//! module, no client library, no `JMAP_FUNCTIONAL_*` path — only
//! `dbus-run-session` and `gnome-keyring-daemon`, so unlike its siblings it
//! runs under a plain `cargo test -p jmap-functional --test secret-store`
//! too. It is still registered as a `functional`-labelled ctest (see
//! `cmake/Functional.cmake`) so the same `-DENABLE_FUNCTIONAL_TESTS=ON`
//! build that catches a missing EDS runtime also catches a missing
//! `gnome-keyring`.

use jmap_functional::Session;
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn a_session_creates_a_usable_login_keyring_before_running_its_client() {
    let session = Session::new(concat!(env!("CARGO_TARGET_TMPDIR"), "/secret-store"));

    // `/bin/true` stands in for "the real client" — this test is about the
    // step `run()` takes before that, not about any client's own behaviour.
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
    // The finding: "`jmap_functional::Session::run` leaks a
    // `gnome-keyring-daemon` per run" — `--daemonize` detaches it from the
    // private bus, so it outlives `dbus-run-session` unless `run` itself
    // reaps it.
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

/// Every `gnome-keyring-daemon` process whose own `XDG_RUNTIME_DIR`
/// environment variable is `runtime_directory` — the same match
/// `jmap-backend-core/tests/secret_store.rs` makes for the same daemon.
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
