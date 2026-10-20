// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The harness for the headless functional tests. It starts a real
//! `evolution-source-registry` and a real host for the module under test:
//! `evolution-addressbook-factory`, `evolution-calendar-factory`, or (for the
//! Camel provider) the client program itself. The host loads a real build of
//! the modules of this repository and talks to an in-process mock JMAP server.
//!
//! All other tests in this workspace stop at the edge of EDS. They call a
//! vfunc body directly, or compare a mapping with a fixture. These tests
//! examine the layer that a user sees first: when EDS calls those vfuncs, and
//! what EDS does with the results. A backend can pass all unit tests and
//! still give an address book that silently refuses writes.
//!
//! # What a session is
//!
//! [`Session`] is a temporary EDS installation in a directory:
//!
//! - A scratch `XDG_CONFIG_HOME`/`XDG_DATA_HOME`/`XDG_CACHE_HOME`. The run
//!   cannot read or damage the Evolution data of the developer, and each run
//!   starts with an empty meta-backend cache. The empty cache is necessary:
//!   `EBookMetaBackend` connects during the open only if it never connected
//!   before. With a reused cache, the connect path races a background refresh.
//! - A scratch module directory that `EDS_ADDRESS_BOOK_MODULES`,
//!   `EDS_CALENDAR_MODULES` or `EDS_CAMEL_PROVIDER_DIR` names. It holds only
//!   the module under test. Nothing is installed system-wide and no `sudo`
//!   is necessary.
//! - A private session bus from `dbus-run-session`. The daemons that D-Bus
//!   activates are the daemons of this test, with the environment of this
//!   test, and they stop with the bus when the client exits. On the session
//!   bus of the developer, a test can silently reach a factory that is
//!   already running with the wrong environment.
//!
//! # What it does not do
//!
//! It does not assert. The client program is a usual libebook consumer that
//! reports what EDS told it. The mock records what the backend requested from
//! the server. The test files make all decisions about this evidence.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

/// A scratch EDS installation: XDG directories, a sources directory, and a
/// module directory, all under one root. The root is removed when a session
/// is created, not when it is dropped, so a failed run keeps its tree for
/// examination.
pub struct Session {
    root: PathBuf,
    environment: BTreeMap<OsString, OsString>,
}

impl Session {
    /// Create a session at `root`. If `root` exists, remove it first.
    ///
    /// Give `env!("CARGO_TARGET_TMPDIR")` joined with a name unique to the
    /// test. Two sessions with the same root share a registry cache.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        if root.exists() {
            fs::remove_dir_all(&root).expect("clear the previous session's tree");
        }

        let mut environment = BTreeMap::new();
        // Do not inherit the environment of the caller. It contains
        // DBUS_SESSION_BUS_ADDRESS (the session bus of the developer and its
        // daemons) and XDG variables that point to real Evolution data.
        //
        // PATH is the exception: `dbus-run-session` uses it to exec
        // `dbus-daemon`. The activated services have `/usr/libexec` paths in
        // their `.service` files, but they start child processes of their own.
        environment.insert(
            "PATH".into(),
            std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
        );
        // The tests read EDS error messages. A translated message does not
        // match.
        environment.insert("LC_ALL".into(), "C".into());

        for (variable, directory) in [
            ("HOME", ""),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_RUNTIME_DIR", "runtime"),
        ] {
            let path = root.join(directory);
            fs::create_dir_all(&path).expect("create the session's XDG directory");
            environment.insert(variable.into(), path.clone().into_os_string());
        }

        // D-Bus refuses a runtime directory that other users can read. It
        // gives only a warning, and the bus then puts its socket in an
        // unexpected location.
        fs::set_permissions(root.join("runtime"), fs::Permissions::from_mode(0o700))
            .expect("lock down the session's XDG_RUNTIME_DIR");

        let session = Self { root, environment };
        fs::create_dir_all(session.sources_directory()).expect("create the sources directory");
        session
    }

    /// Where `evolution-source-registry` reads `.source` keyfiles from.
    pub fn sources_directory(&self) -> PathBuf {
        self.root.join("config/evolution/sources")
    }

    /// Where a collection backend writes its *children*. This is a different
    /// directory: `collection_backend_new_user_file` (EDS 3.52.3,
    /// `e-collection-backend.c:176-200`) puts the child file under
    /// `e_collection_backend_get_cache_dir()`, which is
    /// `$XDG_CACHE_HOME/evolution/sources/<collection-uid>`
    /// (`collection_backend_constructed`, line 927).
    ///
    /// Thus the keyfile of a child source is never in
    /// [`Self::sources_directory`]. For a child source, "this UID matches no
    /// configured source" is normal and is not evidence of debris.
    /// `tests/stale-source-uid.rs` uses this to examine both directories.
    pub fn cache_sources_directory(&self, collection_uid: &str) -> PathBuf {
        self.root
            .join("cache/evolution/sources")
            .join(collection_uid)
    }

    /// Set one more environment variable for the daemons and the client this
    /// session starts.
    ///
    /// The session environment is not inherited (see [`Self::new`]). Thus
    /// this is the only way to enable EDS diagnostics, for example
    /// `ESR_DEBUG=1`. `e_source_registry_debug_enabled`
    /// (`e-data-server-util.c:2372`) controls each
    /// `e_source_registry_debug_print` with it, which includes the
    /// `Failed to lookup password for source …` line that
    /// `tests/stale-source-uid.rs` examines.
    ///
    /// The output goes to the same captured stdout as the `key=value`
    /// observations of the client, because the `dbus-daemon` of
    /// `dbus-run-session` gives its stdout to each service that it activates.
    /// This is safe: the EDS debug lines that contain `=` are keyfile diffs
    /// of the form `" + : Key=Value"`. Their "key" starts with a space, so it
    /// cannot be the same as an observation key.
    pub fn set_variable(&mut self, name: &str, value: &str) {
        self.environment.insert(name.into(), value.into());
    }

    /// Write a `.source` keyfile. The file name is the source UID, which EDS
    /// and the client program use to identify the source.
    pub fn write_source(&self, uid: &str, contents: &str) {
        fs::write(
            self.sources_directory().join(format!("{uid}.source")),
            contents,
        )
        .expect("write the source keyfile");
    }

    /// Write an input file for a client program, and return its path.
    ///
    /// Use this for input that the test supplies, for example a `VTIMEZONE`
    /// that the test also examines. The file is under the session root, so
    /// the next run removes it and a failed run keeps it with its tree.
    pub fn write_input(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, contents).expect("write the client's input file");
        path
    }

    /// Stage a built cdylib as the only address book backend of the session
    /// factory, with the name that EDS derives from `BackendName`.
    pub fn stage_address_book_backend(&mut self, built_module: &Path) {
        self.stage_backend(
            "EDS_ADDRESS_BOOK_MODULES",
            "addressbook-backends",
            "libebookbackendjmap.so",
            built_module,
        );
    }

    /// Do the same for the calendar factory, which uses a different variable
    /// and directory.
    pub fn stage_calendar_backend(&mut self, built_module: &Path) {
        self.stage_backend(
            "EDS_CALENDAR_MODULES",
            "calendar-backends",
            "libecalbackendjmap.so",
            built_module,
        );
    }

    /// Do the same for `evolution-source-registry`, which scans
    /// `EDS_REGISTRY_MODULES` for the collection backend
    /// (`module-jmap-backend.so`, see `docs/manual-test-collection-backend.md`).
    pub fn stage_collection_backend(&mut self, built_module: &Path) {
        self.stage_backend(
            "EDS_REGISTRY_MODULES",
            "registry-modules",
            "module-jmap-backend.so",
            built_module,
        );
    }

    /// Stage an installed EDS registry module next to the module that
    /// [`Self::stage_collection_backend`] staged.
    ///
    /// `EDS_REGISTRY_MODULES` *replaces* the EDS module directory
    /// (`e-source-registry-server.c:1073-1076` assigns it over
    /// `MODULE_DIRECTORY`). Thus a session that stages only our module has no
    /// EDS modules. This is the correct default, but some tests need an EDS
    /// module: `tests/oauth2-stale-proxy.rs` needs `module-oauth2-services.so`.
    /// Its `EOAuth2SourceMonitor` exports the `Source.OAuth2Support` D-Bus
    /// interface for an account whose `[Authentication] Method` names a
    /// registered `EOAuth2Service`.
    ///
    /// The argument is the path of the *installed* EDS module, because a copy
    /// in this repository is a copy of the thing under test. CMake finds it
    /// (see `cmake/Functional.cmake`), so a machine without it fails the
    /// configure step with a clear message.
    pub fn stage_installed_registry_module(&mut self, installed_module: &Path) {
        let name = installed_module
            .file_name()
            .expect("the installed module path names a file");
        // Do not use `stage_backend`: it replaces the directory contents.
        // Add to the directory that holds the collection backend.
        let directory = self.root.join("registry-modules");
        fs::create_dir_all(&directory).expect("create the registry module directory");
        fs::copy(installed_module, directory.join(name)).unwrap_or_else(|error| {
            panic!(
                "copy {} into the session's registry module directory: {error}",
                installed_module.display()
            )
        });
        self.environment
            .insert("EDS_REGISTRY_MODULES".into(), directory.into_os_string());
    }

    /// Stage a built cdylib as the only Camel mail provider of the session,
    /// with the `.urls` file that makes Camel open it.
    ///
    /// No daemon scans this directory. The *client* process dlopens the
    /// provider when a request uses a protocol that a `.urls` file names.
    /// The test copies the `.urls` file that the build installs. A test with
    /// its own copy continues to pass if the installed file loses the protocol.
    pub fn stage_camel_provider(&mut self, built_module: &Path, urls: &Path) {
        self.stage_backend(
            "EDS_CAMEL_PROVIDER_DIR",
            "camel-providers",
            "libcameljmap.so",
            built_module,
        );

        let directory = self.root.join("camel-providers");
        fs::copy(urls, directory.join("libcameljmap.urls"))
            .unwrap_or_else(|error| panic!("copy {} beside the provider: {error}", urls.display()));
    }

    /// Stage a built cdylib in a new scratch directory and return the
    /// directory. The client program loads it with
    /// `e_module_load_all_in_directory`. No daemon loads
    /// `module-jmap-configuration.so`: its `JmapConfigLookup` registers with
    /// a live `EConfigLookup`, which only a client creates. Thus give the
    /// directory to the client, not through an environment variable.
    ///
    /// `LD_LIBRARY_PATH` is not necessary. The `build.rs` of
    /// `jmap-config-module` records the Evolution library directory in the
    /// `RUNPATH` of the module, as in real Evolution modules. Thus `dlopen`
    /// finds the transitive `libevolution-mail.so` dependency.
    pub fn stage_config_lookup_module(&mut self, built_module: &Path) -> PathBuf {
        let directory = self.root.join("config-lookup-module");
        fs::create_dir_all(&directory).expect("create the config-lookup module directory");
        let installed = directory.join("libjmap_config_module.so");
        fs::copy(built_module, &installed).unwrap_or_else(|error| {
            panic!(
                "copy {} into the session's config-lookup module directory: {error}",
                built_module.display()
            )
        });
        directory
    }

    /// Copy `built_module` into a scratch directory of the session and set
    /// `variable` to that directory.
    ///
    /// The variables *replace* the module directory of their host, so the
    /// host has only this module. Thus "no backend factory for jmap" or "no
    /// provider available for protocol" is clear evidence about this module.
    fn stage_backend(
        &mut self,
        variable: &str,
        subdirectory: &str,
        installed_name: &str,
        built_module: &Path,
    ) {
        let directory = self.root.join(subdirectory);
        fs::create_dir_all(&directory).expect("create the backend directory");
        let installed = directory.join(installed_name);
        fs::copy(built_module, &installed).unwrap_or_else(|error| {
            panic!(
                "copy {} into the session's backend directory: {error}",
                built_module.display()
            )
        });
        self.environment
            .insert(variable.into(), directory.into_os_string());
    }

    /// The login keyring file that `gnome-keyring-daemon` (from [`Self::run`])
    /// writes for the session secret store. It exists only after a client ran.
    pub fn login_keyring_file(&self) -> PathBuf {
        self.root.join("data/keyrings/login.keyring")
    }

    /// The unique `XDG_RUNTIME_DIR` of the session. [`Self::reap_keyring_daemon`]
    /// uses it to find daemons. A test can use it to make sure that no daemon
    /// from [`Self::run`] is still alive.
    pub fn runtime_directory(&self) -> PathBuf {
        self.root.join("runtime")
    }

    /// Run `program` on a private session bus with the session environment,
    /// and return its output. The bus and all daemons that it activated stop
    /// before this function returns.
    ///
    /// Before `program` runs, this unlocks a login keyring (or creates it, in
    /// a new session) on the same bus with an empty password. For each account
    /// with an `[Authentication]` extension, the EDS credential lookup asks an
    /// `org.freedesktop.secrets` provider. This includes the
    /// `stored_password_of` of `create_resource_sync`, the resolution in
    /// `authenticate_sync`, and each real password store that the client
    /// stages. Without this step, there are two failures:
    ///
    /// - A container with only `evolution-data-server`/`dbus-daemon` has no
    ///   provider.
    /// - With `gnome-keyring` installed, D-Bus activates `gnome-keyring-daemon`
    ///   (`--start --foreground`, from its `.service` file). It must create or
    ///   unlock a new collection, and it shows a GTK prompt. The headless bus
    ///   has no display, so the call fails in approximately 20ms with "cannot
    ///   open display" (manually confirmed).
    ///
    /// `--daemonize` detaches the keyring daemon from the process group, so it
    /// continues to run after `dbus-run-session` and this function. Thus this
    /// function stops it before it returns. It finds the daemon by its unique
    /// `XDG_RUNTIME_DIR` in `/proc/<pid>/environ`, as
    /// `jmap-backend-core/tests/secret_store.rs` does.
    pub fn run(&self, program: &Path, arguments: &[&str]) -> Output {
        // `sh -c SCRIPT ARG0 ARG1...`: ARG0 becomes $0 and the rest becomes
        // "$@" for the `exec`. Thus the script text never contains the path.
        let unlock_and_exec = r#"
            printf '\n' | gnome-keyring-daemon --daemonize --unlock --components=secrets >/dev/null ||
                { echo "gnome-keyring-daemon --unlock failed; is gnome-keyring installed? EDS D-Bus-activates org.freedesktop.secrets on credential lookups and needs a running keyring daemon to answer it." >&2; exit 97; }
            exec "$0" "$@"
        "#;
        let stdout_path = self.root.join("run.stdout");
        let stderr_path = self.root.join("run.stderr");
        // Use files, not pipes. `Command::output()` waits for end-of-file on
        // stdout, which comes only when all processes with the write end exit.
        // The `dbus-daemon` and the keyring daemon inherit that handle and
        // live longer than `dbus-run-session`. With files, this waits only for
        // the exit status of the shell and cannot hang on a daemon.
        let status = Command::new("dbus-run-session")
            .arg("--")
            .arg("sh")
            .arg("-c")
            .arg(unlock_and_exec)
            .arg(program)
            .args(arguments)
            .env_clear()
            .envs(&self.environment)
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout_path).expect("create the run's stdout file"))
            .stderr(fs::File::create(&stderr_path).expect("create the run's stderr file"))
            .status()
            .unwrap_or_else(|error| {
                panic!("run {} under dbus-run-session: {error}", program.display())
            });
        self.reap_keyring_daemon();
        Output {
            status,
            stdout: fs::read(&stdout_path).expect("read the run's stdout"),
            stderr: fs::read(&stderr_path).expect("read the run's stderr"),
        }
    }

    /// Stop the `gnome-keyring-daemon` that [`Self::run`] started, if it is
    /// alive, and wait for it to exit. The match uses the `XDG_RUNTIME_DIR` of
    /// the session, so it cannot stop a daemon of a different session or of
    /// the developer.
    fn reap_keyring_daemon(&self) {
        for pid in self.keyring_daemons() {
            // SAFETY: a plain signal to a pid that `keyring_daemons` read from
            // `/proc`. The filter keeps only processes whose `XDG_RUNTIME_DIR`
            // is the scratch directory of this session. No other process can
            // have that value.
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
        // SIGTERM does not guarantee an exit, so poll for it. A keyring daemon
        // needs much less than half a second to stop.
        for _ in 0..50 {
            if self.keyring_daemons().is_empty() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// All `gnome-keyring-daemon` processes whose `XDG_RUNTIME_DIR` is the
    /// scratch runtime directory of the session.
    fn keyring_daemons(&self) -> Vec<i32> {
        // `contains` matches both `gnome-keyring-daemon` (from the `run`
        // script) and `/usr/bin/gnome-keyring-daemon` (from D-Bus activation).
        self.pids_matching(b"gnome-keyring-daemon")
    }

    /// All processes whose `cmdline` contains `needle` and whose
    /// `XDG_RUNTIME_DIR` is the scratch runtime directory of the session.
    /// Thus a match is always a daemon that the private bus of *this*
    /// session activated.
    fn pids_matching(&self, needle: &[u8]) -> Vec<i32> {
        let Some(runtime_directory) = self.environment.get(OsStr::new("XDG_RUNTIME_DIR")) else {
            return Vec::new();
        };
        let mut marker = b"XDG_RUNTIME_DIR=".to_vec();
        marker.extend_from_slice(runtime_directory.as_bytes());

        let Ok(entries) = fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| {
                let pid = entry.file_name().to_str()?.parse::<i32>().ok()?;
                let cmdline = fs::read(entry.path().join("cmdline")).ok()?;
                let environ = fs::read(entry.path().join("environ")).ok()?;
                (contains(&cmdline, needle)
                    && environ
                        .split(|byte| *byte == 0)
                        .any(|value| value == marker.as_slice()))
                .then_some(pid)
            })
            .collect()
    }

    /// Like [`Self::run`], but also return the highest `VmHWM` (peak resident
    /// set size) of a process that matches `process_name`, from the private
    /// bus of the session, while `program` ran.
    ///
    /// Use this to measure a D-Bus-activated *factory* process
    /// (`evolution-addressbook-factory`, `evolution-calendar-factory`), not
    /// `program`. The factory holds the meta-backend cache, and it stops with
    /// the private bus, so only this function can examine it. A background
    /// thread polls `/proc` every 20ms, with the `XDG_RUNTIME_DIR` match that
    /// [`Self::keyring_daemons`] uses.
    ///
    /// The value is `None` if the process did not start, for example if its
    /// name is incorrect or the open did not reach the factory.
    pub fn run_measuring_peak_rss(
        &self,
        program: &Path,
        arguments: &[&str],
        process_name: &str,
    ) -> (Output, Option<u64>) {
        let peak = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let needle = process_name.as_bytes().to_vec();

        let output = std::thread::scope(|scope| {
            let peak = &peak;
            let stop = &stop;
            let needle = &needle;
            scope.spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    for pid in self.pids_matching(needle) {
                        if let Some(kb) = process_peak_rss_kb(pid) {
                            peak.fetch_max(kb, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            });
            let output = self.run(program, arguments);
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            output
        });

        let peak = peak.load(std::sync::atomic::Ordering::Relaxed);
        (output, (peak > 0).then_some(peak))
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// The `VmHWM` (peak resident set size, in kB) of `pid`, from
/// `/proc/<pid>/status`. `None` after the process exits, or without `/proc`
/// (all runners of this crate are Linux).
fn process_peak_rss_kb(pid: i32) -> Option<u64> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")
            .and_then(|rest| rest.trim().strip_suffix("kB"))
            .and_then(|value| value.trim().parse().ok())
    })
}

/// A path that CTest gives to the test. CTest knows the CMake output and the
/// EDS runtime locations. If the variable is absent, the test started with a
/// plain `cargo test`, which this crate does not support (see
/// `docs/functional-tests.md`). Then panic with a clear message.
pub fn required_path(variable: &str) -> PathBuf {
    let value = std::env::var_os(variable).unwrap_or_else(|| {
        panic!(
            "{variable} is unset. The functional tests are run by CTest, which passes it:\n  \
             cmake -S . -B build -DENABLE_FUNCTIONAL_TESTS=ON && cmake --build build && \
             ctest --test-dir build -L functional"
        )
    });
    let path = PathBuf::from(value);
    assert!(
        path.exists(),
        "{variable} points at {path:?}, which does not exist"
    );
    path
}

/// Put the `key=value` lines into a map, and ignore all other output. The
/// client programs report their observations in this format.
pub fn observations(stdout: &str) -> BTreeMap<&str, &str> {
    stdout
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect()
}

/// Forward each connection on a new loopback port to `target`, and return
/// the port. `target` is an address that `TcpStream::connect` accepts, for
/// example the bracketed-IPv6 authority of a JMAP origin.
///
/// The `connect_target` of `jmap-backend-core` refuses plaintext to a host
/// that is not loopback (the mock tests use `Host=127.0.0.1`). A `.source`
/// keyfile names a host and port, not a client object as
/// `jmap_client::Client::connect` accepts. Without this proxy, tests against
/// a real server are thus not possible. The proxy obeys the rule: the
/// plaintext traffic of the backend stays on the loopback interface. The
/// plaintext connection to the real server is the same connection that the
/// other live-server tests make directly.
pub fn spawn_loopback_proxy(target: String) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a loopback proxy port");
    let port = listener
        .local_addr()
        .expect("the bound address has a port")
        .port();
    std::thread::spawn(move || {
        for incoming in listener.incoming() {
            let Ok(incoming) = incoming else { continue };
            let target = target.clone();
            std::thread::spawn(move || relay(incoming, &target));
        }
    });
    port
}

/// Copy both directions of one proxied connection. Stop silently when one
/// side closes.
fn relay(mut incoming: std::net::TcpStream, target: &str) {
    let Ok(mut outgoing) = std::net::TcpStream::connect(target) else {
        return;
    };
    let (Ok(mut incoming_reader), Ok(mut outgoing_writer)) =
        (incoming.try_clone(), outgoing.try_clone())
    else {
        return;
    };
    let upstream =
        std::thread::spawn(move || std::io::copy(&mut incoming_reader, &mut outgoing_writer));
    let _ = std::io::copy(&mut outgoing, &mut incoming);
    let _ = upstream.join();
}
