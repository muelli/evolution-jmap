// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests the manual recipe and its hand-written `.source` keyfiles. A recipe is
// prose, so nothing else fails when a group name or the backend name in it is
// wrong. For a collection, the symptom is quiet: with `BackendName=jmapp`, the
// account has no children and shows no error.
//
// The keyfiles are the files in `docs/examples/` that the recipe tells the
// reader to copy. The tests read them as the registry does.
// `evolution-source-registry` calls `e_server_side_source_new` on a GFile for
// each file in its sources directory. That call needs no bus and no running
// daemon. Thus the EDS keyfile code parses the files, and the tests give the
// result to the `collection_source` functions that `populate` calls.
//
// The recipe has two runs, and this file covers both. The second run has an
// account with `MailEnabled=true` and three hand-written mail sources. The
// backend does not create these sources; it only gives them a server. Thus
// those tests call `follow_collection` on the documented files. Then they read
// the transport through the `jmap-mail` `ServerConfig`, as
// `tests/mail_child.rs` does.

use std::ffi::{CStr, c_char};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use eds_sys::{
    CAMEL_NETWORK_SECURITY_METHOD_NONE, CamelNetworkSecurityMethod, CamelSettings,
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_COLLECTION,
    E_SOURCE_EXTENSION_MAIL_ACCOUNT, E_SOURCE_EXTENSION_MAIL_SUBMISSION,
    E_SOURCE_EXTENSION_MAIL_TRANSPORT, ESource, ESourceBackend, ESourceCamel, ESourceMailAccount,
    ESourceMailSubmission, camel_network_settings_get_security_method,
    e_source_backend_get_backend_name, e_source_camel_generate_subtype,
    e_source_camel_get_extension_name, e_source_camel_get_settings, e_source_collection_get_type,
    e_source_get_extension, e_source_get_parent, e_source_get_uid, e_source_has_extension,
    e_source_mail_account_get_identity_uid, e_source_mail_account_get_type,
    e_source_mail_submission_get_transport_uid, e_source_mail_submission_get_type,
};
use glib_sys::GFALSE;
use jmap_backend_collection::child_added::follow_collection;
use jmap_backend_collection::collection_source::{parts_of, server_of, user_of};
use jmap_backend_collection::factory::FACTORY_NAME;
use jmap_backend_collection::mail_child::mail_service_of;
use jmap_backend_collection::prepare_mail::MAIL_BACKEND_NAME;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::source::{ConnectTarget, RegistrySource, SourceError};
use jmap_collection_sync::Parts;
use jmap_mail::server::ServerConfig;
use jmap_mail::settings::settings_type;

mod common;
use common::{with_timeout, with_timeout_duration};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Each keyfile that the recipe tells the reader to copy, in the order of the
/// document. Each `.source` file matches one ```` ```ini ```` block.
///
/// The first is the account of the contacts-and-calendars run. The other four
/// are for the mail run: an account and the three sources of a mail account.
const KEYFILES: [&str; 5] = [
    "jmap-mock-collection.source",
    "jmap-mock-mail-collection.source",
    "jmap-mock-mail-account.source",
    "jmap-mock-mail-identity.source",
    "jmap-mock-mail-transport.source",
];

/// The account of the mail run, and its three child sources.
const MAIL_COLLECTION: &str = KEYFILES[1];
const MAIL_STORE: &str = KEYFILES[2];
const MAIL_IDENTITY: &str = KEYFILES[3];
const MAIL_TRANSPORT: &str = KEYFILES[4];

/// The repository root, found from the crate directory.
fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("the crate directory is inside the repository")
}

fn recipe() -> PathBuf {
    repository().join("docs/manual-test-collection-backend.md")
}

fn example(name: &str) -> PathBuf {
    repository().join("docs/examples").join(name)
}

fn keyfile() -> PathBuf {
    example(KEYFILES[0])
}

/// Whether `repository()` found the real checkout, not a partial copy.
///
/// A tool may copy only `rust/` into a scratch directory and build there
/// (`cargo mutants` does this for its baseline run). `repository()` then
/// still returns a path, but a wrong one: the fixtures in `docs/` do not
/// exist at it. Every real checkout has `.git` at its root and no such copy
/// does, so that is the check this function makes.
fn in_full_checkout() -> bool {
    repository().join(".git").exists()
}

/// Skips the calling test unless `in_full_checkout()` is true. Call first,
/// not after reading a fixture: a missing fixture inside a real checkout
/// must still fail the test, not skip it.
macro_rules! require_full_checkout {
    () => {
        if !in_full_checkout() {
            eprintln!(
                "skipping {}: not a full checkout, docs/ fixtures are unavailable \
                 (for example, a cargo-mutants scratch copy)",
                module_path!()
            );
            return;
        }
    };
}

/// Generates the `ESourceCamel` subtype for `jmap` and returns its extension
/// name, `[JMAP Backend]`.
///
/// This helper is the same as the one in `tests/mail_child.rs`. The provider is
/// linked into the test, not installed, so nothing calls
/// `e_source_camel_register_types()`. Two threads that generate the subtype at
/// the same time cause an abort in GObject, so a `OnceLock` guards it.
fn camel_extension_name() -> *const c_char {
    static NAME: OnceLock<usize> = OnceLock::new();
    let name = *NAME.get_or_init(|| {
        // SAFETY: a NUL-terminated protocol name and a GType derived from
        // CamelSettings, which `settings_type` registers. The returned name is
        // interned and is never freed.
        unsafe {
            let gtype =
                e_source_camel_generate_subtype(MAIL_BACKEND_NAME.as_ptr(), settings_type());
            assert_ne!(
                gtype, 0,
                "no ESourceCamel subtype was generated for the jmap protocol"
            );
            e_source_camel_get_extension_name(MAIL_BACKEND_NAME.as_ptr()) as usize
        }
    });
    name as *const c_char
}

/// A documented `.source` file, loaded as `evolution-source-registry` loads it.
struct Example(RegistrySource);

impl Example {
    fn load(path: &Path) -> Self {
        Self(RegistrySource::load(path))
    }

    fn as_ptr(&self) -> *mut ESource {
        self.0.as_ptr()
    }

    /// The uid that the registry gives this file: its name without the
    /// extension. The `Parent=` of a child file must use this string.
    fn uid(&self) -> Option<String> {
        // SAFETY: a live source; the getter returns a string owned by it.
        unsafe { read_string(e_source_get_uid(self.as_ptr())) }
    }

    /// The uid of the parent that this source names.
    fn parent(&self) -> Option<String> {
        // SAFETY: as above.
        unsafe { read_string(e_source_get_parent(self.as_ptr())) }
    }

    fn has(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a header constant.
        unsafe { e_source_has_extension(self.as_ptr(), name.as_ptr()) != GFALSE }
    }

    /// The `CamelSettings` object that a Camel service gets from this source.
    /// As in `tests/mail_child.rs`, this does the two steps of
    /// `e_source_camel_configure_service` without the service.
    fn camel_settings(&self) -> *mut CamelSettings {
        // SAFETY: a live source and the interned extension name of a registered
        // `ESourceCamel` subtype. The source creates and owns the extension and
        // its settings object.
        unsafe {
            let extension: *mut ESourceCamel =
                e_source_get_extension(self.as_ptr(), camel_extension_name()).cast();
            assert!(!extension.is_null(), "the jmap subtype is not registered");
            let settings = e_source_camel_get_settings(extension);
            assert!(!settings.is_null(), "the extension holds no settings");
            settings
        }
    }

    /// The `CamelNetworkSecurityMethod` converted from `[Security] Method`. A
    /// wrong spelling fails silently in this conversion, so the test reads the
    /// result, not the string.
    fn camel_security_method(&self) -> CamelNetworkSecurityMethod {
        // SAFETY: the settings object of a live source, which implements
        // `CamelNetworkSettings`.
        unsafe { camel_network_settings_get_security_method(self.camel_settings().cast()) }
    }

    /// The server that `jmap-mail` reads from the settings. Its `connect_sync`
    /// needs this server to build a client.
    fn server(&self) -> Result<ServerConfig, SourceError> {
        // SAFETY: the settings object of a live source, only read from.
        unsafe { ServerConfig::from_settings(self.camel_settings()) }
    }

    /// The backend name in the `[Collection]` group.
    fn backend_name(&self) -> Option<String> {
        // SAFETY: a reference to the GType of the extension registers it, so the
        // lookup below can find it. Then a header constant that names an
        // extension owned by the source. The call can create the extension.
        // `collection_source` avoids this, but a temporary copy of the file
        // can accept it.
        let name = unsafe {
            e_source_collection_get_type();
            let extension =
                e_source_get_extension(self.as_ptr(), E_SOURCE_EXTENSION_COLLECTION.as_ptr())
                    .cast::<ESourceBackend>();
            e_source_backend_get_backend_name(extension)
        };
        if name.is_null() {
            return None;
        }
        // SAFETY: a non-NULL return is a NUL-terminated string that the
        // extension owns. The extension outlives this copy.
        Some(
            unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}

#[test]
fn the_recipes_keyfile_describes_the_mock_server() {
    require_full_checkout!();
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let source = Example::load(&keyfile());
        // SAFETY: a source that is alive during each call, which is the only
        // requirement of the `collection_source` functions.
        let (server, user) = unsafe { (server_of(source.as_ptr()), user_of(source.as_ptr())) };

        // The default port of `jmap-mockd`, without TLS. The shared `origin` rules
        // allow plaintext only for loopback.
        assert_eq!(
            server
                .expect("the documented account names a server")
                .target,
            ConnectTarget::Origin("http://127.0.0.1:8080".into())
        );
        // No user, intentionally. With a user, `populate` asks EDS for a password
        // before a connection, so the first run shows a prompt, not a fan-out.
        // `jmap-mockd` without `--basic` or `--bearer` needs no credentials.
        assert_eq!(user, None);
    });
}

/// The three switches that decide what a populate lists.
///
/// The account of the first run has mail off, because the backend does not
/// create mail sources. The Evolution account assistant creates them, and the
/// second run writes them by hand. `Parts::any` is true if one of the three
/// parts is on.
#[test]
fn the_recipes_keyfile_switches_on_the_parts_the_backend_fans_out_to() {
    require_full_checkout!();
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        let source = Example::load(&keyfile());
        // SAFETY: as above.
        let parts = unsafe { parts_of(source.as_ptr()) };

        assert_eq!(
            parts,
            Parts {
                mail: false,
                contacts: true,
                calendars: true,
            }
        );
        assert!(
            parts.any(),
            "an account with no part enabled is one whose populate does nothing at \
             all, which is not what this recipe is testing"
        );
    });
}

#[test]
fn the_recipes_keyfile_names_the_backend_the_factory_answers_to() {
    require_full_checkout!();
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // The registry files each collection factory under
        // `"<factory_name>:Collection"` and looks up the key built from this string.
        // A mismatch is silent: `e_source_registry_server_ref_backend_factory`
        // returns NULL and the account has no children.
        assert_eq!(
            Example::load(&keyfile()).backend_name().as_deref(),
            Some(FACTORY_NAME.to_str().expect("the factory name is UTF-8"))
        );
    });
}

#[test]
fn the_recipe_quotes_every_keyfile_verbatim() {
    require_full_checkout!();
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // The reader copies the files but reads the document, so the two must
        // agree. Each `ini` block in the document is a full keyfile, quoted
        // exactly. The fragment that shows the groups a mail source gets has no
        // language tag, because nobody copies it.
        let recipe = fs::read_to_string(recipe()).expect("the recipe is in docs/");
        let quoted = fenced_blocks(&recipe, "ini");

        assert_eq!(
            quoted.len(),
            KEYFILES.len(),
            "expected one ini block per keyfile in docs/examples/, found {}",
            quoted.len()
        );
        for (block, name) in quoted.iter().zip(KEYFILES) {
            assert_eq!(
                block,
                &fs::read_to_string(example(name)).expect("the keyfile is in docs/examples/"),
                "docs/manual-test-collection-backend.md and docs/examples/{name} disagree"
            );
        }
    });
}

#[test]
fn the_mail_runs_account_switches_on_every_part() {
    require_full_checkout!();
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // The important difference of the second account is `MailEnabled=true`.
        // `collection_backend_bind_child_enabled()` binds the `enabled` of each mail
        // source to the `mail-enabled` of the account. Under an account with mail
        // off, the three hand-written sources load disabled. They are in
        // `~/.config/evolution/sources/`, but not in Evolution, and no error shows.
        let account = Example::load(&example(MAIL_COLLECTION));

        // SAFETY: a source that is alive during the call, which is the only
        // requirement of the `collection_source` functions.
        assert_eq!(
            unsafe { parts_of(account.as_ptr()) },
            Parts {
                mail: true,
                contacts: true,
                calendars: true,
            }
        );
        assert_eq!(
            account.backend_name().as_deref(),
            Some(FACTORY_NAME.to_str().expect("the factory name is UTF-8"))
        );
        // SAFETY: a source that is alive during each call.
        let (server, user) = unsafe { (server_of(account.as_ptr()), user_of(account.as_ptr())) };
        assert_eq!(
            server
                .expect("the documented account names a server")
                .target,
            ConnectTarget::Origin("http://127.0.0.1:8080".into())
        );
        assert_eq!(user, None);
    });
}

#[test]
fn the_documented_mail_sources_hang_off_the_documented_account() {
    require_full_checkout!();
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // `child_added` sees these files only because of `Parent=`. EDS emits
        // `child-added` on the collection whose uid a source names. Here, a uid is
        // a file name. With a typo, Evolution shows the account without the three
        // sources, and logs nothing.
        let account = Example::load(&example(MAIL_COLLECTION));
        let uid = account
            .uid()
            .expect("a source loaded from a file has a uid");

        for name in [MAIL_STORE, MAIL_IDENTITY, MAIL_TRANSPORT] {
            assert_eq!(
                Example::load(&example(name)).parent().as_deref(),
                Some(uid.as_str()),
                "docs/examples/{name} is parented to something else"
            );
        }
    });
}

#[test]
fn the_documented_mail_sources_point_at_each_other() {
    require_full_checkout!();
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // When the user sends, Evolution follows the chain that `prepare_mail`
        // describes. The receiving account names the identity, and the identity
        // names its transport. The files contain these uids as hand-written text,
        // so the uids can be wrong.
        let store = Example::load(&example(MAIL_STORE));
        let identity = Example::load(&example(MAIL_IDENTITY));
        let transport = Example::load(&example(MAIL_TRANSPORT));

        // SAFETY: a reference to the GTypes registers them, so the lookups below
        // can find the extensions. Each extension is present on its source, so
        // nothing is created here.
        let (named_identity, named_transport) = unsafe {
            e_source_mail_account_get_type();
            e_source_mail_submission_get_type();
            let account: *mut ESourceMailAccount =
                e_source_get_extension(store.as_ptr(), E_SOURCE_EXTENSION_MAIL_ACCOUNT.as_ptr())
                    .cast();
            let submission: *mut ESourceMailSubmission = e_source_get_extension(
                identity.as_ptr(),
                E_SOURCE_EXTENSION_MAIL_SUBMISSION.as_ptr(),
            )
            .cast();
            (
                read_string(e_source_mail_account_get_identity_uid(account)),
                read_string(e_source_mail_submission_get_transport_uid(submission)),
            )
        };

        assert_eq!(named_identity, identity.uid());
        assert_eq!(named_transport, transport.uid());
    });
}

#[test]
fn the_documented_services_are_this_accounts_and_the_identity_is_not() {
    require_full_checkout!();
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // `mail_service_of`, which the registry callback calls, decides which of
        // the three sources `child_added` writes a server on. Thus two files need
        // `BackendName=jmap`, and the identity intentionally has none.
        for (name, extension) in [
            (MAIL_STORE, E_SOURCE_EXTENSION_MAIL_ACCOUNT),
            (MAIL_TRANSPORT, E_SOURCE_EXTENSION_MAIL_TRANSPORT),
        ] {
            let service = Example::load(&example(name));
            // SAFETY: a live source.
            assert_eq!(
                unsafe { mail_service_of(service.as_ptr()) },
                Some(extension),
                "docs/examples/{name} is not a mail service of this account"
            );
        }

        let identity = Example::load(&example(MAIL_IDENTITY));
        // SAFETY: a live source.
        assert_eq!(unsafe { mail_service_of(identity.as_ptr()) }, None);
    });
}

#[test]
fn the_documented_transport_reaches_the_mock_server() {
    require_full_checkout!();
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // The transport that the reader writes has a provider and no server. After
        // the registry reads it, its Camel settings must name the mock server. The
        // test reads them with the `jmap-mail` reader that `connect_sync` calls.
        let account = Example::load(&example(MAIL_COLLECTION));

        for name in [MAIL_STORE, MAIL_TRANSPORT] {
            let service = Example::load(&example(name));
            assert!(
                !service.has(E_SOURCE_EXTENSION_AUTHENTICATION),
                "docs/examples/{name} was written with a server of its own, so this \
                 test would pass without the binding it is checking"
            );

            // SAFETY: two live sources.
            unsafe { follow_collection(account.as_ptr(), service.as_ptr()) };

            assert_eq!(
                service.server(),
                Ok(ServerConfig {
                    target: jmap_backend_core::source::ConnectTarget::Origin(
                        "http://127.0.0.1:8080".to_owned()
                    ),
                    user: None,
                }),
                "docs/examples/{name} does not reach the mock server"
            );
            // The mock server has no TLS. A mail source that requests TLS does not
            // connect.
            assert_eq!(
                service.camel_security_method(),
                CAMEL_NETWORK_SECURITY_METHOD_NONE,
            );
        }
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_recipe_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}

/// The contents of each ```` ```<language> ```` block in `markdown`.
fn fenced_blocks<'a>(markdown: &'a str, language: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<Vec<&'a str>> = None;
    for line in markdown.lines() {
        match (&mut current, line.trim_end()) {
            (None, fence) if fence == format!("```{language}") => current = Some(Vec::new()),
            (Some(lines), "```") => {
                let mut block = lines.join("\n");
                block.push('\n');
                blocks.push(block);
                current = None;
            }
            (Some(lines), _) => lines.push(line),
            (None, _) => {}
        }
    }
    blocks
}
