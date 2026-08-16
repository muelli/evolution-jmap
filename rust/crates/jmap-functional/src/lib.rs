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
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
    /// Also sets `LD_LIBRARY_PATH` to `/usr/lib/evolution`: the module's
    /// transitive `libevolution-mail.so` dependency lives there, off the
    /// default loader path, and `dlopen`ing the module later does not inherit
    /// `evolution-shell-3.0`'s own `-Wl,-R` rpath the way linking against it
    /// directly would — found by the 307th session's hand-driven spike
    /// (`docs/NIGHT-LOG.md`) before this method existed.
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
        self.environment
            .insert("LD_LIBRARY_PATH".into(), "/usr/lib/evolution".into());
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

    /// Run `program` on a private session bus in this session's environment,
    /// and return what it said. The bus — and every daemon activated on it —
    /// is gone by the time this returns.
    pub fn run(&self, program: &Path, arguments: &[&str]) -> Output {
        Command::new("dbus-run-session")
            .arg("--")
            .arg(program)
            .args(arguments)
            .env_clear()
            .envs(&self.environment)
            .output()
            .unwrap_or_else(|error| {
                panic!("run {} under dbus-run-session: {error}", program.display())
            })
    }
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
