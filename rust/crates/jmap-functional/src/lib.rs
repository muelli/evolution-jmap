// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The harness for the headless functional tests (M9 layer 1): a real
//! `evolution-source-registry` and a real host for the module under test —
//! `evolution-addressbook-factory`, `evolution-calendar-factory`, or, for the
//! Camel provider, the client program itself — loading a real build of this
//! repository's modules and talking to an in-process mock JMAP server.
//!
//! Every other test in this workspace stops at the edge of EDS — it calls a
//! vfunc body directly, or checks a mapping against a fixture. That leaves
//! one layer untested, and it is the layer a user meets first: EDS deciding
//! *when* to call those vfuncs and what to do with what they said. A backend
//! can pass every unit test in the tree and still give an address book that
//! silently refuses writes, because nothing below this file ever asks EDS
//! what it made of the backend's answers.
//!
//! # What a session is
//!
//! [`Session`] is a throwaway EDS installation in a directory:
//!
//! - a scratch `XDG_CONFIG_HOME`/`XDG_DATA_HOME`/`XDG_CACHE_HOME`, so the
//!   run cannot see — or corrupt — the developer's own Evolution data, and
//!   so every run starts with an empty meta-backend cache. The cache being
//!   empty is load-bearing, not hygiene: `EBookMetaBackend` connects during
//!   the open only when it has never connected before, so a reused cache
//!   would make the connect path race with a background refresh.
//! - a scratch module directory named by `EDS_ADDRESS_BOOK_MODULES`,
//!   `EDS_CALENDAR_MODULES` or `EDS_CAMEL_PROVIDER_DIR`, holding the one
//!   module under test and nothing else. Nothing is installed system-wide and
//!   no `sudo` is involved.
//! - a private session bus from `dbus-run-session`, so the daemons that
//!   D-Bus activates are this test's daemons, started with this test's
//!   environment, and are killed with the bus when the client exits. A test
//!   that used the developer's session bus would silently reach an
//!   already-running factory started with the wrong environment.
//!
//! # What it deliberately does not do
//!
//! It does not assert. The client program is an ordinary libebook consumer
//! that reports what EDS told it; the mock records what the backend asked
//! the server for. Both are evidence, and the test files hold every
//! judgement about it.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

/// A scratch EDS installation: XDG directories, a sources directory, and a
/// module directory, all under one root that is wiped when the session is
/// created rather than when it is dropped — a failed run leaves its tree
/// behind to be looked at.
pub struct Session {
    root: PathBuf,
    environment: BTreeMap<OsString, OsString>,
}

impl Session {
    /// Create a session rooted at `root`, which is removed if it exists.
    ///
    /// Pass `env!("CARGO_TARGET_TMPDIR")` joined with a name unique to the
    /// test: two sessions sharing a root would share a registry cache.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        if root.exists() {
            fs::remove_dir_all(&root).expect("clear the previous session's tree");
        }

        let mut environment = BTreeMap::new();
        // Deliberately not the caller's environment. Inheriting it would
        // bring DBUS_SESSION_BUS_ADDRESS, which points at the developer's
        // own session bus and at the daemons already running on it, and the
        // XDG variables pointing at their real Evolution data.
        //
        // PATH is the exception: `dbus-run-session` execs `dbus-daemon`
        // through it, and the activated services are `/usr/libexec` paths in
        // their `.service` files but run child processes of their own.
        environment.insert(
            "PATH".into(),
            std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
        );
        // Error messages from EDS reach the client, and this test reads
        // them; a translated one would be read as a different message.
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

        // D-Bus refuses a runtime directory anyone else can read, and says
        // so as a warning rather than a failure — which would leave the bus
        // running with a socket somewhere unexpected.
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

    /// Write a `.source` keyfile. The file name is the source UID, which is
    /// how EDS identifies it and how the client program asks for it.
    pub fn write_source(&self, uid: &str, contents: &str) {
        fs::write(
            self.sources_directory().join(format!("{uid}.source")),
            contents,
        )
        .expect("write the source keyfile");
    }

    /// Write a file for a client program to read, and hand back its path.
    ///
    /// For input a client is *told* rather than one it invents — a `VTIMEZONE`
    /// the test also asserts about, say. Written under the session's root so it
    /// goes when the next run wipes it, and so a failed run leaves it beside
    /// the tree it belongs to.
    pub fn write_input(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, contents).expect("write the client's input file");
        path
    }

    /// Stage a built cdylib as the one address book backend this session's
    /// factory can see, under the name EDS derives from `BackendName`.
    pub fn stage_address_book_backend(&mut self, built_module: &Path) {
        self.stage_backend(
            "EDS_ADDRESS_BOOK_MODULES",
            "addressbook-backends",
            "libebookbackendjmap.so",
            built_module,
        );
    }

    /// The same for the calendar factory, which scans a directory of its own
    /// named by a variable of its own.
    pub fn stage_calendar_backend(&mut self, built_module: &Path) {
        self.stage_backend(
            "EDS_CALENDAR_MODULES",
            "calendar-backends",
            "libecalbackendjmap.so",
            built_module,
        );
    }

    /// The same for `evolution-source-registry` itself, which scans
    /// `EDS_REGISTRY_MODULES` for the collection backend
    /// (`module-jmap-backend.so`, per `docs/manual-test-collection-backend.md`)
    /// rather than a factory's own directory.
    pub fn stage_collection_backend(&mut self, built_module: &Path) {
        self.stage_backend(
            "EDS_REGISTRY_MODULES",
            "registry-modules",
            "module-jmap-backend.so",
            built_module,
        );
    }

    /// Stage one of EDS's OWN already-installed registry modules beside the
    /// one [`Self::stage_collection_backend`] staged.
    ///
    /// Needed because `EDS_REGISTRY_MODULES` *replaces* EDS's module directory
    /// rather than adding to it (`e-source-registry-server.c:1073` assigns it
    /// over `MODULE_DIRECTORY`), so a session that stages only our own module
    /// runs a registry with none of EDS's. That is the right default — every
    /// other functional test here wants exactly one backend in play — but it
    /// silently removes behaviour a test may be *about*: `tests/oauth2-stale-
    /// proxy.rs` needs `module-oauth2-services.so`, whose `EOAuth2SourceMonitor`
    /// is what exports the `Source.OAuth2Support` D-Bus interface for an
    /// account whose `[Authentication] Method` names a registered
    /// `EOAuth2Service`.
    ///
    /// Takes the *installed* module's path rather than a built one: this is
    /// EDS's module, not ours, and a copy of it in this repository would be a
    /// copy of the thing under test. CMake finds it (see `cmake/Functional.cmake`)
    /// so that a machine without it fails the configure step by name instead of
    /// producing a test that quietly measures nothing.
    pub fn stage_installed_registry_module(&mut self, installed_module: &Path) {
        let name = installed_module
            .file_name()
            .expect("the installed module path names a file");
        // Not `stage_backend`: that would reset EDS_REGISTRY_MODULES to a
        // fresh directory and drop whatever was staged before it. This adds
        // to the directory the collection backend already went into.
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

    /// Stage a built cdylib as the one Camel mail provider this session can
    /// see, together with the `.urls` file that is what makes Camel open it.
    ///
    /// Unlike the two above, this directory is not scanned by a daemon: the
    /// provider is dlopened in the *client's* own process, and only when
    /// something asks for a protocol one of the `.urls` files in here claims.
    /// So the `.urls` file is staged rather than written — it is the file the
    /// build installs, and a test that wrote its own copy would keep passing
    /// after the installed one stopped naming the protocol.
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

    /// Stage a built cdylib in a scratch directory of its own and return that
    /// directory, for a client program that loads it itself via
    /// `e_module_load_all_in_directory` rather than a directory a daemon
    /// scans. `module-jmap-configuration.so`'s `JmapConfigLookup` is not found
    /// by any daemon this crate stages against elsewhere — it registers
    /// against a live `EConfigLookup`, which only a client constructs — so
    /// the directory has to be handed to that client rather than passed
    /// through the environment the way `EDS_ADDRESS_BOOK_MODULES` and its
    /// siblings are.
    ///
    /// No `LD_LIBRARY_PATH` juggling needed here: `jmap-config-module`'s own
    /// `build.rs` records `/usr/lib/evolution` in the built module's
    /// `RUNPATH`, the same way every real Evolution module has it, so
    /// `dlopen`ing the module resolves its transitive `libevolution-mail.so`
    /// dependency on its own. That was not always so: the 307th session's
    /// hand-driven spike (`docs/NIGHT-LOG.md`) found the module missing that
    /// `RUNPATH` and worked around it here with `LD_LIBRARY_PATH`, without
    /// tracing why the `RUNPATH` itself was absent; a later session found and
    /// fixed the actual cause (see `docs/NIGHT-LOG.md`'s "CURRENT PRIORITY
    /// item 2(a)" entry) and removed the workaround, since a passing test
    /// that still carried it would not prove the fix.
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

    /// Copy `built_module` into a scratch directory of this session's and
    /// point `variable` at it.
    ///
    /// All three of `EDS_ADDRESS_BOOK_MODULES`, `EDS_CALENDAR_MODULES` and
    /// `EDS_CAMEL_PROVIDER_DIR` *replace* their host's module directory rather
    /// than adding to it, so a host started here has this module and no other
    /// — which is why a stray "no backend factory for jmap", or "no provider
    /// available for protocol", is unambiguous evidence about this module.
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

    /// Where `gnome-keyring-daemon` (started by [`Self::run`], below) writes
    /// the login keyring this session's secret store lives in. Exists only
    /// after a client has actually run.
    pub fn login_keyring_file(&self) -> PathBuf {
        self.root.join("data/keyrings/login.keyring")
    }

    /// This session's own `XDG_RUNTIME_DIR`, unique to it — what
    /// [`Self::reap_keyring_daemon`] matches against, and what a test can
    /// use to check that no daemon [`Self::run`] started is still alive.
    pub fn runtime_directory(&self) -> PathBuf {
        self.root.join("runtime")
    }

    /// Run `program` on a private session bus in this session's environment,
    /// and return what it said. The bus — and every daemon activated on it —
    /// is gone by the time this returns.
    ///
    /// Before `program` runs, a login keyring is unlocked (created, on a
    /// fresh session) on the same bus with an empty password. Every account
    /// with an `[Authentication]` extension makes EDS's credential lookup —
    /// `create_resource_sync`'s `stored_password_of`, `authenticate_sync`'s
    /// resolution, and any real password store this session's client stages
    /// — ask a `org.freedesktop.secrets` provider, and without this step
    /// that ask either finds no provider at all (a functional container
    /// carrying only `evolution-data-server`/`dbus-daemon`, no
    /// `gnome-keyring`, per `docs/ROADMAP.md` item 18) or, once one is
    /// installed, hits `gnome-keyring-daemon`'s own default D-Bus activation
    /// (`--start --foreground`, from its `.service` file) needing to CREATE
    /// or UNLOCK a collection it has never seen before — which falls back to
    /// a GTK prompt this headless bus has no display for, failing in
    /// milliseconds rather than merely doing nothing (confirmed by hand: a
    /// secret-store call against such a freshly-activated, keyring-less
    /// service loses to "cannot open display" in ~20ms). Unlocking up front,
    /// before anything asks, avoids both failure modes entirely.
    ///
    /// `--daemonize` detaches the keyring daemon from this call's own
    /// process group, so it outlives `dbus-run-session` and this method:
    /// nothing here waits for the bus to be the daemon's only owner. Left
    /// alone, that is a process leaked onto the machine on every call — this
    /// reaps it (by matching its own `XDG_RUNTIME_DIR`, unique to this
    /// session, against `/proc/<pid>/environ`) before returning, the same
    /// mechanism `jmap-backend-core/tests/secret_store.rs` uses for the same
    /// daemon.
    pub fn run(&self, program: &Path, arguments: &[&str]) -> Output {
        // `sh -c SCRIPT ARG0 ARG1...`: ARG0 becomes $0, the rest become the
        // "$@" the final `exec` forwards — so `program`'s own path never has
        // to survive being interpolated into the script text.
        let unlock_and_exec = r#"
            printf '\n' | gnome-keyring-daemon --daemonize --unlock --components=secrets >/dev/null ||
                { echo "gnome-keyring-daemon --unlock failed; is gnome-keyring installed? (docs/ROADMAP.md item 18)" >&2; exit 97; }
            exec "$0" "$@"
        "#;
        let stdout_path = self.root.join("run.stdout");
        let stderr_path = self.root.join("run.stderr");
        // Files, not pipes: `Command::output()` waits for end-of-file on the
        // child's stdout, which arrives only once every process holding the
        // write end has exited — and the private bus's `dbus-daemon`, plus
        // the keyring daemon it activates, inherit that handle and outlive
        // `dbus-run-session`. Waiting on the shell's own exit status, with
        // its stdout/stderr going to files, is what this call actually
        // means, and it cannot hang on a daemon this method is about to reap
        // anyway.
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

    /// Terminate the `gnome-keyring-daemon` [`Self::run`] started, if it is
    /// still alive, and wait for it to actually go. Matches by this
    /// session's own `XDG_RUNTIME_DIR`, so it can only ever find and kill
    /// this session's own daemon, never another session's or the
    /// developer's.
    fn reap_keyring_daemon(&self) {
        for pid in self.keyring_daemons() {
            // SAFETY: a plain signal to a pid `keyring_daemons` just read out
            // of `/proc`, filtered to processes whose own `XDG_RUNTIME_DIR`
            // is this session's scratch directory — nothing else can have
            // that value.
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
        // SIGTERM is a request, not a guarantee, so poll for the exit this
        // is actually meant to produce rather than assume it. Half a second
        // is far more than a keyring daemon needs to shut down.
        for _ in 0..50 {
            if self.keyring_daemons().is_empty() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Every `gnome-keyring-daemon` process whose own `XDG_RUNTIME_DIR`
    /// environment variable is this session's scratch runtime directory.
    fn keyring_daemons(&self) -> Vec<i32> {
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
                // Both spellings: `gnome-keyring-daemon` as started by the
                // script above, and `/usr/bin/gnome-keyring-daemon` as D-Bus
                // activation would start it.
                let cmdline = fs::read(entry.path().join("cmdline")).ok()?;
                let environ = fs::read(entry.path().join("environ")).ok()?;
                (contains(&cmdline, b"gnome-keyring-daemon")
                    && environ
                        .split(|byte| *byte == 0)
                        .any(|value| value == marker.as_slice()))
                .then_some(pid)
            })
            .collect()
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// A path handed to the test by CTest, which knows where CMake put things
/// and where the EDS runtime is. Absent means the test was started by hand
/// with a plain `cargo test`, which is not a thing this crate supports — see
/// `docs/functional-tests.md` — so say that rather than passing quietly.
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

/// Split `key=value` lines into a map, ignoring anything else the program
/// printed. The client programs report their observations this way.
pub fn observations(stdout: &str) -> BTreeMap<&str, &str> {
    stdout
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect()
}
