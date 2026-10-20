// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `secret_store::default_collection_is_locked` against a real
//! `gnome-keyring-daemon`, in the three states that matter.
//!
//! No fake can prove this. The module exists because EDS's own API cannot see
//! a locked keyring. Thus the proof that a direct Secret Service query *does*
//! see it must come from a secret service that is really locked.
//! `crate::resolver`'s live `_jmap._tcp` lookup has the same standard.
//!
//! # Why every test here re-executes this binary
//!
//! `g_bus_get_sync(G_BUS_TYPE_SESSION, …)` returns a per-process singleton.
//! The first call sets the bus that the process uses for its full life. Thus
//! three scenarios need three processes. Each test runs *this* test binary
//! again, with `JMAP_SECRET_STORE_PROBE` set and `--exact` selecting
//! [`probe`], inside its own `dbus-run-session`. It reads the answer from the
//! stdout of the child. Without the variable, [`probe`] returns immediately.
//! Thus a plain run of this file does not test the keyring that is unlocked
//! on the developer's own bus.
//!
//! # Why they are all `#[ignore]`d
//!
//! They need `dbus-run-session` and `gnome-keyring-daemon`, which
//! `ci/install-deps.sh` does not install. Thus `rust-test-eds`, which runs
//! this crate in the ordinary `build` job, must not run them. Instead, CMake
//! registers them as the `functional`-labelled `functional-secret-store-lock`
//! (`cmake/Functional.cmake`). It is next to the sibling test of the
//! functional harness's own unlock step, in the only job that installs
//! `gnome-keyring`.
//!
//! The scenario that starts a keyring daemon also kills it (matched on the
//! scratch `XDG_RUNTIME_DIR` that no other process has). A `--daemonize`d
//! `gnome-keyring-daemon` continues to run after its bus stops. Without the
//! kill, a test file that starts three daemons per run leaks three processes
//! per run.

use std::env;
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A parent test sets this on the child that it re-executes. It is absent in
/// all other processes.
const PROBE: &str = "JMAP_SECRET_STORE_PROBE";

/// The prefix that [`probe`] prints and that every parent looks for.
const ANSWER: &str = "default-collection-is-locked=";

/// The same, for [`secret_store::service_is_available`]. The same child prints
/// it on the same run, not a separate probe. Both questions go to one bus, so
/// their answers are comparable. The "absent from the bus but startable"
/// scenario below is *defined* by a difference between the two answers.
const ANSWER_AVAILABLE: &str = "service-is-available=";

/// The child part of every test below: report what the module says about the
/// bus of this process, and nothing else.
///
/// `#[ignore]`d like its parents. Also, it does nothing without [`PROBE`]. If
/// not, a `--ignored` run of the full file asks the developer's own session
/// bus about the developer's own keyring.
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
    println!(
        "{ANSWER_AVAILABLE}{:?}",
        jmap_backend_core::secret_store::service_is_available()
    );
}

/// A scratch home for one scenario, with its own `XDG_RUNTIME_DIR`. Thus the
/// keyring daemon of one scenario cannot find or be found by the daemon of
/// another scenario. (`gnome-keyring-daemon` gives control to a running daemon
/// that it finds through the control socket there. Thus a locked scenario can
/// silently use an unlocked daemon.)
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

/// Run [`probe`] on a private session bus in `root`, after `setup`, and
/// return its [`ANSWER`] value. `setup` is a `/bin/sh` fragment that starts
/// the secret service that the scenario needs.
fn probe_in(root: &Path, setup: &str) -> String {
    extract(&run_probe(root, setup, None), ANSWER)
}

/// What [`secret_store::service_is_available`] says on the same bus.
///
/// If given, `servicedir` becomes the *only* directory where the private bus
/// looks for activatable services. That is the purpose of the parameter.
///
/// `XDG_DATA_DIRS` cannot do this. This note exists because it was the first
/// attempt, and it silently passed for the wrong reason.
/// `<standard_session_servicedirs/>` always expands to the XDG directories
/// **plus dbus's own compile-time datadir**. Thus
/// `/usr/share/dbus-1/services/org.freedesktop.secrets.service` stays
/// activatable on any machine with gnome-keyring installed, whatever the
/// environment. A bus with no secret service must therefore come from a config
/// file, which `dbus-run-session --config-file` accepts. [`None`] keeps the
/// stock session config, and thus the system's real services.
fn availability_in(root: &Path, setup: &str, servicedir: Option<&Path>) -> String {
    extract(&run_probe(root, setup, servicedir), ANSWER_AVAILABLE)
}

/// The rest of the line that starts with `prefix`. If there is no such line,
/// a panic that shows the printed output.
fn extract(stdout: &str, prefix: &str) -> String {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .unwrap_or_else(|| panic!("the probe printed no {prefix:?} line\n--- stdout ---\n{stdout}"))
        .to_owned()
}

/// Run [`probe`] on a private session bus and return its full stdout.
fn run_probe(root: &Path, setup: &str, servicedir: Option<&Path>) -> String {
    let binary = env::current_exe().expect("this test binary's own path");
    // `sh -c SCRIPT ARG0 ARG1…`: ARG0 becomes `$0`, so the binary's path is
    // never interpolated into the script text. `jmap_functional::Session::run`
    // uses the same shape for the same reason.
    let script = format!(
        r#"
        {setup}
        {await_service}
        exec "$0" "$@"
        "#,
        await_service = if setup.is_empty() { "" } else { AWAIT_SERVICE },
    );

    // Files, not pipes, and the exit status, not `Command::output()`.
    // `output()` waits for end-of-file on the child's stdout. That occurs only
    // when *every* process that holds the write end has exited. The
    // `dbus-daemon` of a private session bus, and any daemon activated on it,
    // inherits that handle and can outlive `dbus-run-session`. This test must
    // wait only for the shell, and that wait cannot hang on a stray daemon.
    let stdout_path = root.join("probe.out");
    let stderr_path = root.join("probe.err");
    let mut command = Command::new("dbus-run-session");
    if let Some(servicedir) = servicedir {
        let config = root.join("bus.conf");
        fs::write(&config, bus_config(servicedir)).expect("write the private bus config");
        command.arg(format!("--config-file={}", config.display()));
    }
    let status = command
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
        // Intentionally not the caller's environment: it has
        // DBUS_SESSION_BUS_ADDRESS and the XDG variables that point to the
        // developer's own keyring. PATH stays, because `dbus-run-session`
        // uses it to exec `dbus-daemon` and the script uses it to run
        // `gnome-keyring-daemon`.
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
    assert!(
        stdout.contains(ANSWER),
        "the probe printed no answer (exit {status})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    stdout
}

/// A session bus that can activate exactly what is in `servicedir` and
/// nothing else.
///
/// Intentionally *not* `<standard_session_servicedirs/>`: it adds the system's
/// own services again, and this config must exclude them. The policy is the
/// stock session policy, because this bus is private to one test process and
/// owns nothing that needs restriction.
fn bus_config(servicedir: &Path) -> String {
    format!(
        r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <servicedir>{}</servicedir>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
        servicedir.display()
    )
}

/// Terminate the `gnome-keyring-daemon` that this scenario started. The
/// scratch `XDG_RUNTIME_DIR`, which no other process in the table can have,
/// identifies it.
///
/// A `--daemonize`d keyring daemon continues to run after its bus stops. It is
/// not a child of a process that this test can wait for. Without this, each
/// run of this file leaves three daemons running, permanently. This is Rust
/// code, not part of the shell script. The first version was in the script,
/// and it silently did nothing. Here an assertion checks it (in
/// [`an_unlocked_login_keyring_reports_itself_unlocked`]), so a failure does
/// not look like success.
fn reap_keyring_daemons(runtime_directory: &Path) {
    for pid in keyring_daemons(runtime_directory) {
        // SAFETY: a plain signal to a pid that the scan just read. Its
        // `XDG_RUNTIME_DIR` is a directory that this test created, so it
        // cannot be the process of a different user or test.
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
    // SIGTERM is a request, so the tests assert the real result. Half a
    // second is many times more than the time a keyring daemon needs to stop.
    for _ in 0..50 {
        if keyring_daemons(runtime_directory).is_empty() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Every `gnome-keyring-daemon` that has `runtime_directory` as its
/// `XDG_RUNTIME_DIR`. For a scratch directory, these are only the daemons
/// that this scenario started.
fn keyring_daemons(runtime_directory: &Path) -> Vec<i32> {
    let marker = format!("XDG_RUNTIME_DIR={}", runtime_directory.display());
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<i32>().ok()?;
            // Both spellings: `gnome-keyring-daemon` as this file starts
            // it, and `/usr/bin/gnome-keyring-daemon` as D-Bus activation
            // starts it.
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

/// `[u8]::contains` finds a single byte. This finds a subslice.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Wait until the secret service is on the bus. That can be later than the
/// start of its daemon.
///
/// `gnome-keyring-daemon --daemonize` returns after it forks, *before* it
/// gets `org.freedesktop.secrets`. The module under test intentionally never
/// auto-starts a service. Thus a probe in that window reads "no secret
/// service", and the test result becomes random. This was measured: without
/// this wait, the unlocked scenario answered `None`. With it, one 100ms
/// iteration was sufficient.
///
/// This uses `NameHasOwner` because it is the only question that activates
/// nothing. Thus the wait cannot hide a failure of the property that it
/// protects. `dbus-send` is in the same package as `dbus-run-session`, which
/// every test here already needs.
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

/// Unlocks a login keyring with an empty password (and creates it on a fresh
/// scratch tree), the same as `jmap_functional::Session::run` does.
const UNLOCK: &str = r#"
    printf '\n' | gnome-keyring-daemon --daemonize --unlock --components=secrets >/dev/null ||
        { echo "gnome-keyring-daemon --unlock failed; is gnome-keyring installed?" >&2; exit 97; }
"#;

/// The same daemon *without* the unlock. It serves the login keyring that is
/// already on disk, locked, because it has no password. This is the
/// operator's own symptom: a session where PAM never unlocked the keyring.
/// D-Bus activation of `org.freedesktop.secrets` also gives this state on
/// any desktop where the user dismisses the unlock prompt.
const START_LOCKED: &str = r#"
    gnome-keyring-daemon --daemonize --components=secrets >/dev/null ||
        { echo "gnome-keyring-daemon failed to start; is gnome-keyring installed?" >&2; exit 97; }
"#;

/// The state of a working desktop, where nothing must change. A consent
/// request is useful here, so the module must not report a locked store.
#[test]
#[ignore = "needs dbus-run-session and gnome-keyring-daemon; see the module docs"]
fn an_unlocked_login_keyring_reports_itself_unlocked() {
    let root = scratch("unlocked");
    assert_eq!(probe_in(&root, UNLOCK), "Some(false)");
    // This check is necessary. A keyring daemon continues to run after its
    // bus stops. A scenario that does not stop its daemon leaves one process
    // per run, permanently.
    assert_eq!(keyring_daemons(&root.join("run")), Vec::<i32>::new());
}

/// A secret service that is locked, not absent. A locked collection must be a
/// collection, so the test first creates and unlocks a keyring and copies its
/// files. Then a second daemon serves them, on a second bus and a second
/// runtime directory, without the password.
#[test]
#[ignore = "needs dbus-run-session and gnome-keyring-daemon; see the module docs"]
fn a_locked_login_keyring_reports_itself_locked() {
    let seed = scratch("locked-seed");
    // This run exists only to create the keyring files, so only its unlock
    // step is important.
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

/// No secret service on the bus. The answer must be "do not know" and
/// **not** an activation. With a wrong answer, every OAuth 2.0 account on a
/// machine with no keyring reports a locked store and does not ask for
/// consent. An activation adds a 25-second timeout to the connect path.
#[test]
#[ignore = "needs dbus-run-session; see the module docs"]
fn no_secret_service_at_all_is_not_a_locked_store() {
    let root = scratch("absent");
    assert_eq!(probe_in(&root, ""), "None");
}

/// A machine with *no* secret service: none running, and none that the bus
/// can start. `secret_store::service_is_available` exists to identify this
/// state. Here a sign-in window is a trap: the consent succeeds, but the
/// token has no place to go, so the next fetch asks again, forever.
///
/// The private bus config is the full scenario. Without it, the bus finds
/// `/usr/share/dbus-1/services/org.freedesktop.secrets.service` on any
/// machine with gnome-keyring installed and correctly answers "startable".
/// The next test checks that case.
#[test]
#[ignore = "needs dbus-run-session; see the module docs"]
fn a_bus_that_could_not_start_a_secret_service_reports_none_available() {
    let root = scratch("no-service-files");
    let empty = root.join("empty-servicedir");
    fs::create_dir_all(&empty).expect("create a servicedir with no service files");

    assert_eq!(availability_in(&root, "", Some(&empty)), "Some(false)");
}

/// The main distinction of the function: **not running is not the same as
/// not there.** An ordinary desktop that has not needed its keyring yet must
/// behave exactly as it did before this probe existed. Thus a service that
/// the bus can start counts as available, although nothing owns the name.
///
/// Also, the service must not start. `secret_store` promises never to
/// activate anything. (If `evolution-source-registry` activates
/// `org.freedesktop.secrets` and it cannot start, the connect path gets a
/// 25-second timeout.) The stub service here touches a marker file if the bus
/// runs it. Thus the test checks the promise through the absence of that
/// file. An assertion, not a comment.
#[test]
#[ignore = "needs dbus-run-session; see the module docs"]
fn a_startable_secret_service_counts_as_available_and_is_not_started() {
    let root = scratch("startable");
    let services = root.join("servicedir");
    fs::create_dir_all(&services).expect("create the service directory");
    let marker = root.join("it-was-activated");
    fs::write(
        services.join("org.freedesktop.secrets.service"),
        format!(
            "[D-BUS Service]\nName=org.freedesktop.secrets\nExec=/bin/sh -c \"touch '{}'\"\n",
            marker.display()
        ),
    )
    .expect("write the stub service file");

    assert_eq!(availability_in(&root, "", Some(&services)), "Some(true)");
    assert!(
        !marker.exists(),
        "the probe activated the secret service; `service_is_available` must \
         only ever ask the bus questions that start nothing"
    );
}

/// A secret service that is running is available, whatever the state of its
/// collections. A *locked* keyring is still a place to store a token. Thus
/// this answer and the answer of
/// [`a_locked_login_keyring_reports_itself_locked`] differ on the same bus.
#[test]
#[ignore = "needs dbus-run-session and gnome-keyring-daemon; see the module docs"]
fn a_running_secret_service_reports_available() {
    let root = scratch("running-available");
    assert_eq!(availability_in(&root, UNLOCK, None), "Some(true)");
    assert_eq!(keyring_daemons(&root.join("run")), Vec::<i32>::new());
}
