// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `secret_store::default_collection_is_locked` against a real
//! `gnome-keyring-daemon`, in the three states that matter.
//!
//! No fake can prove this one. The whole point of the module
//! (`docs/ROADMAP.md` item 17(a)) is that a locked keyring is invisible
//! through EDS's own API, so the evidence that asking the Secret Service
//! directly *does* see it has to come from a secret service that is really
//! locked — the same standard `crate::resolver`'s live `_jmap._tcp` lookup
//! is held to.
//!
//! # Why every test here re-executes this binary
//!
//! `g_bus_get_sync(G_BUS_TYPE_SESSION, …)` hands out a per-process
//! singleton: the first call fixes which bus the process talks to for its
//! whole life. Three scenarios therefore need three processes, so each test
//! runs *this* test binary again — with `JMAP_SECRET_STORE_PROBE` set and
//! `--exact` selecting [`probe`] — inside a `dbus-run-session` of its own,
//! and reads the answer off its stdout. Without the variable [`probe`]
//! returns immediately, so a plain run of this file is not a run against
//! whoever's keyring happens to be unlocked on the developer's own bus.
//!
//! # Why they are all `#[ignore]`d
//!
//! They need `dbus-run-session` and `gnome-keyring-daemon`, which
//! `ci/install-deps.sh` does not install — so `rust-test-eds`, which runs
//! this crate in the ordinary `build` job, must not run them. CMake
//! registers them as the `functional`-labelled `functional-secret-store-lock`
//! instead (`cmake/Functional.cmake`), beside the sibling test of the
//! functional harness's own unlock step, in the one job that does install
//! `gnome-keyring`.
//!
//! Each scenario's keyring daemon is killed by the scenario that started it
//! (matched on the scratch `XDG_RUNTIME_DIR` no other process has), rather
//! than left behind: a `--daemonize`d `gnome-keyring-daemon` outlives the
//! bus it was started on, so a test file that ran three of them per run
//! would leak three processes per run.

use std::env;
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Set by a parent test on the child it re-executes; absent everywhere else.
const PROBE: &str = "JMAP_SECRET_STORE_PROBE";

/// What [`probe`] prints and every parent looks for.
const ANSWER: &str = "default-collection-is-locked=";

/// The child half of every test below: report what the module says about the
/// bus this process was started on, and nothing else.
///
/// `#[ignore]`d like its parents, and additionally inert without [`PROBE`] —
/// a `--ignored` run of the whole file would otherwise ask the developer's
/// own session bus about the developer's own keyring.
#[test]
#[ignore = "re-executed by the tests below inside a private session bus"]
fn probe() {
    if env::var_os(PROBE).is_none() {
        return;
    }
    println!(
        "{ANSWER}{:?}",
        jmap_backend_core::secret_store::default_collection_is_locked()
    );
}

/// A scratch home for one scenario: its own `XDG_RUNTIME_DIR`, so the
/// keyring daemon it starts neither finds nor is found by any other
/// scenario's (`gnome-keyring-daemon` hands over to an already-running
/// daemon it discovers through the control socket there, which would make a
/// locked scenario silently reuse an unlocked daemon).
fn scratch(name: &str) -> PathBuf {
    let root = PathBuf::from(concat!(env!("CARGO_TARGET_TMPDIR"), "/secret-store")).join(name);
    if root.exists() {
        fs::remove_dir_all(&root).expect("clear the previous run's tree");
    }
    for directory in ["data", "config", "cache", "run"] {
        fs::create_dir_all(root.join(directory)).expect("create the scratch tree");
    }
    // gnome-keyring-daemon refuses a world-readable runtime directory.
    fs::set_permissions(root.join("run"), fs::Permissions::from_mode(0o700))
        .expect("lock down the scratch runtime directory");
    root
}

/// Run [`probe`] on a private session bus in `root`, after `setup` — a
/// `/bin/sh` fragment that starts whatever secret service the scenario wants
/// — and answer what it said.
fn probe_in(root: &Path, setup: &str) -> String {
    let binary = env::current_exe().expect("this test binary's own path");
    // `sh -c SCRIPT ARG0 ARG1…`: ARG0 becomes `$0`, so the binary's path
    // never has to survive being interpolated into the script text — the
    // same shape `jmap_functional::Session::run` uses for the same reason.
    let script = format!(
        r#"
        {setup}
        {await_service}
        exec "$0" "$@"
        "#,
        await_service = if setup.is_empty() { "" } else { AWAIT_SERVICE },
    );

    // Files rather than pipes, and the exit status rather than
    // `Command::output()`. `output()` waits for end-of-file on the child's
    // stdout, which arrives only when *every* process holding the write end
    // has exited — and a private session bus's `dbus-daemon`, plus any
    // daemon activated on it, inherits that handle and can outlive
    // `dbus-run-session`. Waiting on the shell alone is what this test
    // actually means, and it cannot hang on a stray daemon.
    let stdout_path = root.join("probe.out");
    let stderr_path = root.join("probe.err");
    let status = Command::new("dbus-run-session")
        .arg("--")
        .arg("sh")
        .arg("-c")
        .arg(&script)
        .arg(&binary)
        .args(["--exact", "probe", "--ignored", "--nocapture"])
        .stdin(Stdio::null())
        .stdout(File::create(&stdout_path).expect("create the probe's stdout file"))
        .stderr(File::create(&stderr_path).expect("create the probe's stderr file"))
        .env_clear()
        // Deliberately not the caller's environment: it carries
        // DBUS_SESSION_BUS_ADDRESS and the XDG variables pointing at the
        // developer's own keyring. PATH stays because `dbus-run-session`
        // execs `dbus-daemon` through it and the script runs
        // `gnome-keyring-daemon` through it.
        .env(
            "PATH",
            env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
        )
        .env("HOME", root)
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .env(PROBE, "1")
        .status()
        .expect("run the probe under dbus-run-session (is dbus-run-session installed?)");
    reap_keyring_daemons(&root.join("run"));

    let stdout = fs::read_to_string(&stdout_path).expect("read the probe's stdout");
    let stderr = fs::read_to_string(&stderr_path).expect("read the probe's stderr");
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(ANSWER))
        .unwrap_or_else(|| {
            panic!(
                "the probe printed no answer (exit {status})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
            )
        })
        .to_owned()
}

/// Terminate the `gnome-keyring-daemon` this scenario started, identified by
/// the scratch `XDG_RUNTIME_DIR` no other process in the table can have.
///
/// A `--daemonize`d keyring daemon outlives the bus it was started on — it
/// is not a child of anything this test can wait for — so without this a run
/// of this file would leave three of them behind, every time, forever. Done
/// here rather than in the shell script — which is where it was first
/// written, and where it silently did nothing — so that it is code with an
/// assertion behind it (in
/// [`an_unlocked_login_keyring_reports_itself_unlocked`]) rather than a
/// fragment whose failure looks exactly like success.
fn reap_keyring_daemons(runtime_directory: &Path) {
    for pid in keyring_daemons(runtime_directory) {
        // SAFETY: a plain signal to a pid the scan just read, whose own
        // `XDG_RUNTIME_DIR` is a directory this test created — so it cannot
        // be anybody else's process.
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
    // SIGTERM is a request, so the assertion the tests make is about what
    // actually happened. Half a second is several orders of magnitude more
    // than a keyring daemon needs to shut down.
    for _ in 0..50 {
        if keyring_daemons(runtime_directory).is_empty() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Every `gnome-keyring-daemon` running with `runtime_directory` as its
/// `XDG_RUNTIME_DIR`, which for a scratch directory is "the ones this
/// scenario started" and nothing else.
fn keyring_daemons(runtime_directory: &Path) -> Vec<i32> {
    let marker = format!("XDG_RUNTIME_DIR={}", runtime_directory.display());
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<i32>().ok()?;
            // Both spellings: `gnome-keyring-daemon` as started here, and
            // `/usr/bin/gnome-keyring-daemon` as D-Bus activation starts it.
            let cmdline = fs::read(entry.path().join("cmdline")).ok()?;
            let environ = fs::read(entry.path().join("environ")).ok()?;
            (contains(&cmdline, b"gnome-keyring-daemon")
                && environ
                    .split(|byte| *byte == 0)
                    .any(|v| v == marker.as_bytes()))
            .then_some(pid)
        })
        .collect()
}

/// `[u8]::contains` is for a single byte; this is the subslice question.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Wait for the secret service to actually be on the bus, which is not the
/// same moment as its daemon having started.
///
/// `gnome-keyring-daemon --daemonize` returns once it has forked, *before*
/// it has acquired `org.freedesktop.secrets` — and the module under test
/// deliberately never auto-starts a service, so a probe run inside that
/// window reads "no secret service" and the test becomes a coin toss.
/// Measured rather than feared: without this the unlocked scenario answered
/// `None`; with it, a single 100ms iteration was enough.
///
/// `NameHasOwner` is used precisely because it is the one question that
/// activates nothing, so waiting cannot quietly paper over the very flag it
/// is here to protect. `dbus-send` ships in the same package as the
/// `dbus-run-session` every test here already needs.
const AWAIT_SERVICE: &str = r#"
    waited=0
    until dbus-send --session --print-reply --dest=org.freedesktop.DBus \
            /org/freedesktop/DBus org.freedesktop.DBus.NameHasOwner \
            string:org.freedesktop.secrets 2>/dev/null | grep -q "boolean true"; do
        waited=$((waited + 1))
        if [ "$waited" -gt 50 ]; then
            echo "org.freedesktop.secrets never reached the private bus" >&2
            exit 98
        fi
        sleep 0.1
    done
"#;

/// Unlocks — and on a fresh scratch tree, creates — a login keyring with an
/// empty password, exactly as `jmap_functional::Session::run` does.
const UNLOCK: &str = r#"
    printf '\n' | gnome-keyring-daemon --daemonize --unlock --components=secrets >/dev/null ||
        { echo "gnome-keyring-daemon --unlock failed; is gnome-keyring installed?" >&2; exit 97; }
"#;

/// The same daemon *without* the unlock: it serves the login keyring already
/// on disk, locked, because nothing has given it a password. This is the
/// operator's own symptom (`docs/ROADMAP.md` item 17) — a session whose
/// keyring PAM never unlocked — and it is also what D-Bus activation of
/// `org.freedesktop.secrets` produces on any desktop where the unlock prompt
/// is dismissed.
const START_LOCKED: &str = r#"
    gnome-keyring-daemon --daemonize --components=secrets >/dev/null ||
        { echo "gnome-keyring-daemon failed to start; is gnome-keyring installed?" >&2; exit 97; }
"#;

/// The state a working desktop is in, and the one where nothing should
/// change: consent is worth asking for, so the module must not report a
/// locked store.
#[test]
#[ignore = "needs dbus-run-session and gnome-keyring-daemon; see the module docs"]
fn an_unlocked_login_keyring_reports_itself_unlocked() {
    let root = scratch("unlocked");
    assert_eq!(probe_in(&root, UNLOCK), "Some(false)");
    // Not incidental hygiene: a keyring daemon survives the bus it was
    // started on, so a scenario that did not clean up after itself would
    // leave one process behind per run, indefinitely.
    assert_eq!(keyring_daemons(&root.join("run")), Vec::<i32>::new());
}

/// The case item 17(a) exists for. Seeded first — a locked collection has to
/// be a collection, so a keyring is created and unlocked once and its files
/// copied — then served by a second daemon, on a second bus and a second
/// runtime directory, that was never given the password.
#[test]
#[ignore = "needs dbus-run-session and gnome-keyring-daemon; see the module docs"]
fn a_locked_login_keyring_reports_itself_locked() {
    let seed = scratch("locked-seed");
    // `/bin/true` for a client: this run exists only to create the keyring
    // files, so it is the unlock step alone that is wanted from it.
    assert_eq!(probe_in(&seed, UNLOCK), "Some(false)");

    let root = scratch("locked");
    let keyrings = root.join("data/keyrings");
    fs::create_dir_all(&keyrings).expect("create the keyring directory");
    for entry in fs::read_dir(seed.join("data/keyrings")).expect("read the seeded keyrings") {
        let entry = entry.expect("a seeded keyring file");
        fs::copy(entry.path(), keyrings.join(entry.file_name())).expect("copy a seeded keyring");
    }

    assert_eq!(probe_in(&root, START_LOCKED), "Some(true)");
}

/// No secret service on the bus at all. The answer must be "do not know" and
/// **not** an activation: a wrong answer here would have every OAuth 2.0
/// account on a machine with no keyring report a locked store instead of
/// asking for consent, and an activation would put item 18's 25-second
/// timeout on the connect path.
#[test]
#[ignore = "needs dbus-run-session; see the module docs"]
fn no_secret_service_at_all_is_not_a_locked_store() {
    let root = scratch("absent");
    assert_eq!(probe_in(&root, ""), "None");
}
