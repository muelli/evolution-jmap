// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// M6's last acceptance criterion is the same as M3's and M4's: a documented
// manual recipe with a hand-written `.source` keyfile. A recipe is prose, and
// nothing else in this repository fails when a group name in it is wrong or when
// the backend name it tells the reader to write stops being the one the factory
// answers to. For a *collection* the symptom is quieter than for an address
// book, because the registry has a factory for every account it cannot place:
// `BackendName=jmapp` is an account that simply has no children, with no error
// anywhere.
//
// So the keyfiles are files — the ones in `docs/examples/` the recipe says to
// copy — and these tests read them the way the registry does.
// `e_server_side_source_new` on a GFile is exactly what
// `evolution-source-registry` calls for every file in its sources directory, and
// it needs neither a bus nor a running daemon; what the reader copies is
// therefore parsed by EDS's own keyfile code and handed to the same
// `collection_source` functions `populate` calls.
//
// The recipe has two runs and this file covers both. The second — an account
// with `MailEnabled=true` and the three mail sources hand-written under it —
// exists because the mail half of this backend has no other visible surface:
// nothing here creates those sources, and what the backend does for them is
// give them a server. So those tests go one step further than the first run's
// and call `follow_collection` on the documented files, then read the transport
// back through `jmap-mail`'s own `ServerConfig` — the same far-end assertion
// `tests/mail_child.rs` makes, over the files a reader actually copies rather
// than over sources a test built.

use std::ffi::{CStr, c_char};
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::OnceLock;

use eds_sys::{
    CAMEL_NETWORK_SECURITY_METHOD_NONE, CamelNetworkSecurityMethod, CamelSettings,
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_COLLECTION,
    E_SOURCE_EXTENSION_MAIL_ACCOUNT, E_SOURCE_EXTENSION_MAIL_SUBMISSION,
    E_SOURCE_EXTENSION_MAIL_TRANSPORT, ESource, ESourceBackend, ESourceCamel, ESourceMailAccount,
    ESourceMailSubmission, ESourceRegistryServer, camel_network_settings_get_security_method,
    e_server_side_source_new, e_source_backend_get_backend_name, e_source_camel_generate_subtype,
    e_source_camel_get_extension_name, e_source_camel_get_settings, e_source_collection_get_type,
    e_source_get_extension, e_source_get_parent, e_source_get_uid, e_source_has_extension,
    e_source_mail_account_get_identity_uid, e_source_mail_account_get_type,
    e_source_mail_submission_get_transport_uid, e_source_mail_submission_get_type,
    e_source_registry_server_new, g_file_new_for_path, g_object_unref,
};
use glib_sys::GFALSE;
use jmap_backend_collection::child_added::follow_collection;
use jmap_backend_collection::collection_source::{parts_of, server_of, user_of};
use jmap_backend_collection::factory::FACTORY_NAME;
use jmap_backend_collection::mail_child::mail_service_of;
use jmap_backend_collection::prepare_mail::MAIL_BACKEND_NAME;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::source::{ConnectTarget, SourceError};
use jmap_collection_sync::Parts;
use jmap_mail::server::ServerConfig;
use jmap_mail::settings::settings_type;

mod common;
use common::{with_timeout, with_timeout_duration};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Every keyfile the recipe tells the reader to copy, in the order the document
/// quotes them — one `.source` file per ```` ```ini ```` block, checked against
/// the block below.
///
/// The first is the account of the contacts-and-calendars run; the other four
/// are the mail run's, which is an account plus the three sources a mail account
/// is made of.
const KEYFILES: [&str; 5] = [
    "jmap-mock-collection.source",
    "jmap-mock-mail-collection.source",
    "jmap-mock-mail-account.source",
    "jmap-mock-mail-identity.source",
    "jmap-mock-mail-transport.source",
];

/// The mail run's account, and the three sources parented to it.
const MAIL_COLLECTION: &str = KEYFILES[1];
const MAIL_STORE: &str = KEYFILES[2];
const MAIL_IDENTITY: &str = KEYFILES[3];
const MAIL_TRANSPORT: &str = KEYFILES[4];

/// The repository root, from the crate this test lives in.
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

/// The name `[JMAP Backend]` — the extension a `jmap` service's Camel settings
/// live under — having first generated the `ESourceCamel` subtype that carries
/// it.
///
/// The same helper, for the same reason and with the same `OnceLock`, as
/// `tests/mail_child.rs`: the provider is linked into this test rather than
/// installed, so nothing has called `e_source_camel_register_types()` and the one
/// subtype these tests need is generated directly. Losing the race between
/// generating and registering it is an abort inside GObject, and Rust runs the
/// tests in this file as threads of one process.
fn camel_extension_name() -> *const c_char {
    static NAME: OnceLock<usize> = OnceLock::new();
    let name = *NAME.get_or_init(|| {
        // SAFETY: a NUL-terminated protocol name and a GType derived from
        // CamelSettings, which is what `settings_type` registers; the name it
        // hands back is interned and never freed.
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

/// A `.source` file loaded the way `evolution-source-registry` loads one.
///
/// The same stand-in as `jmap-backend-book`'s and `jmap-backend-cal`'s recipe
/// tests, and a third copy rather than a shared helper: cargo gives each
/// integration test its own binary, and the alternative — a support crate in the
/// workspace whose only user is three test files — would be a published-looking
/// crate that exists to hold thirty lines.
struct RegistrySource {
    server: *mut gobject_sys::GObject,
    source: *mut ESource,
}

impl RegistrySource {
    /// Parses `path`.
    ///
    /// `e_server_side_source_new` insists on a registry server — it is the object
    /// a server-side source reports changes to — but constructing one neither
    /// owns a bus name nor reads the user's sources; that only happens when it is
    /// run. So this is the whole daemon-free part of the daemon, and it is the
    /// part that turns a keyfile into an `ESource`.
    fn load(path: &Path) -> Self {
        let path_string =
            std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).expect("no NUL in a path");
        let mut error = ptr::null_mut();
        // SAFETY: the constructor takes no arguments; the path is NUL-terminated
        // and copied; the GFile is owned here and released below.
        let (server, source) = unsafe {
            // `e_source_registry_server_new` is declared as returning its own
            // base class, `EDBusServer *`, the way EDS's own callers use it; the
            // cast back down is the one C does implicitly.
            let server = e_source_registry_server_new().cast::<ESourceRegistryServer>();
            let file = g_file_new_for_path(path_string.as_ptr());
            let source = e_server_side_source_new(server, file, &mut error);
            g_object_unref(file.cast());
            (server, source)
        };
        assert!(
            !source.is_null(),
            "the registry could not read {}: {}",
            path.display(),
            // SAFETY: a NULL return means the GError was set.
            unsafe { CStr::from_ptr((*error).message) }.to_string_lossy()
        );
        Self {
            server: server.cast(),
            source,
        }
    }

    /// The uid the registry gives this file, which is its name without the
    /// extension — and so the string another file's `Parent=` has to spell.
    fn uid(&self) -> Option<String> {
        // SAFETY: a live source; the getter returns a string owned by it.
        unsafe { read_string(e_source_get_uid(self.source)) }
    }

    /// What this source says it hangs off.
    fn parent(&self) -> Option<String> {
        // SAFETY: as above.
        unsafe { read_string(e_source_get_parent(self.source)) }
    }

    fn has(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a header constant.
        unsafe { e_source_has_extension(self.source, name.as_ptr()) != GFALSE }
    }

    /// The `CamelSettings` object a Camel service configured from this source
    /// would be given — `e_source_camel_configure_service`'s own two steps with
    /// the service left out, exactly as `tests/mail_child.rs` does it.
    fn camel_settings(&self) -> *mut CamelSettings {
        // SAFETY: a live source and the interned extension name of a registered
        // `ESourceCamel` subtype; the extension is created on demand and owned
        // by the source, and so is the settings object it holds.
        unsafe {
            let extension: *mut ESourceCamel =
                e_source_get_extension(self.source, camel_extension_name()).cast();
            assert!(!extension.is_null(), "the jmap subtype is not registered");
            let settings = e_source_camel_get_settings(extension);
            assert!(!settings.is_null(), "the extension holds no settings");
            settings
        }
    }

    /// The `CamelNetworkSecurityMethod` those settings arrived at — the value
    /// the string in `[Security] Method` was converted into rather than the
    /// string itself, because the conversion is where a wrong spelling goes
    /// quiet.
    fn camel_security_method(&self) -> CamelNetworkSecurityMethod {
        // SAFETY: the settings object of a live source, which implements
        // `CamelNetworkSettings`.
        unsafe { camel_network_settings_get_security_method(self.camel_settings().cast()) }
    }

    /// The server `jmap-mail` reads off those settings — the last thing its
    /// `connect_sync` needs before it builds a client.
    fn server(&self) -> Result<ServerConfig, SourceError> {
        // SAFETY: the settings object of a live source, only read from.
        unsafe { ServerConfig::from_settings(self.camel_settings()) }
    }

    /// What the `[Collection]` group says the backend is called.
    fn backend_name(&self) -> Option<String> {
        // SAFETY: referencing the extension's GType registers it, so that the
        // lookup below can find it in a test binary that has not otherwise
        // touched a collection source. Then a header constant naming an extension
        // the source owns — created rather than merely fetched, which is what
        // `collection_source` is careful not to do and what a throwaway copy of
        // the file may.
        let name = unsafe {
            e_source_collection_get_type();
            let extension =
                e_source_get_extension(self.source, E_SOURCE_EXTENSION_COLLECTION.as_ptr())
                    .cast::<ESourceBackend>();
            e_source_backend_get_backend_name(extension)
        };
        if name.is_null() {
            return None;
        }
        // SAFETY: a non-NULL return is a NUL-terminated string owned by the
        // extension, which outlives the copy taken here.
        Some(
            unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}

impl Drop for RegistrySource {
    fn drop(&mut self) {
        // SAFETY: we hold the only reference to each.
        unsafe {
            g_object_unref(self.source.cast());
            g_object_unref(self.server);
        }
    }
}

#[test]
fn the_recipes_keyfile_describes_the_mock_server() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let source = RegistrySource::load(&keyfile());
        // SAFETY: a source alive for the length of each call, which is all
        // `collection_source`'s functions ask for.
        let (server, user) = unsafe { (server_of(source.source), user_of(source.source)) };

        // `jmap-mockd`'s default port, in the clear, which the shared `origin` rules
        // allow for loopback and nothing else.
        assert_eq!(
            server
                .expect("the documented account names a server")
                .target,
            ConnectTarget::Origin("http://127.0.0.1:8080".into())
        );
        // No user, on purpose. An account that names one makes `populate` ask EDS to
        // resolve a password before anything is contacted, and the recipe's first run
        // would be a prompt rather than a fan-out; `jmap-mockd` with no
        // `--basic`/`--bearer` wants no credentials at all.
        assert_eq!(user, None);
    });
}

/// The three switches that decide what a populate lists — and the reason the
/// recipe's account has mail off.
///
/// This backend creates no mail children yet, so `MailEnabled=true` would be an
/// account that claims a part nothing serves. It is not harmless prose: `mail`
/// is one of the three bits `Parts::any` is a disjunction over, so it is the
/// difference between an account whose populate contacts the server and one that
/// returns having done nothing.
#[test]
fn the_recipes_keyfile_switches_on_the_parts_the_backend_fans_out_to() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let source = RegistrySource::load(&keyfile());
        // SAFETY: as above.
        let parts = unsafe { parts_of(source.source) };

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
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The registry files each collection factory under
        // `"<factory_name>:Collection"` and asks for the key built from this string.
        // A mismatch is silent — `e_source_registry_server_ref_backend_factory`
        // answers NULL and the account keeps sitting there with no children.
        assert_eq!(
            RegistrySource::load(&keyfile()).backend_name().as_deref(),
            Some(FACTORY_NAME.to_str().expect("the factory name is UTF-8"))
        );
    });
}

#[test]
fn the_recipe_quotes_every_keyfile_verbatim() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The reader copies the files, but reads the document; if the two drift
        // apart, whichever one they trusted is the wrong one. So an `ini` block in
        // this document is a whole keyfile quoted verbatim and nothing else — the
        // fragment showing what a mail source *grows* is fenced without a language,
        // because it is not a file anybody copies.
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
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The other account in the recipe, and the one difference that matters:
        // `MailEnabled=true`. EDS binds each mail source's `enabled` to the
        // account's `mail-enabled` in `collection_backend_bind_child_enabled()`, so
        // the three sources the reader hand-writes under an account with mail off
        // are three sources that arrive disabled — present in
        // `~/.config/evolution/sources/`, absent from Evolution, and with no error
        // anywhere saying why.
        let account = RegistrySource::load(&example(MAIL_COLLECTION));

        // SAFETY: a source alive for the length of the call, which is all
        // `collection_source`'s functions ask for.
        assert_eq!(
            unsafe { parts_of(account.source) },
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
        // SAFETY: a source alive for the length of each call.
        let (server, user) = unsafe { (server_of(account.source), user_of(account.source)) };
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
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `Parent=` is the whole reason `child_added` ever sees these files: EDS
        // emits `child-added` on the collection whose uid a source names, and a uid
        // is a file name here. A typo is three sources that belong to nothing —
        // Evolution shows the account without them, and nothing is logged.
        let account = RegistrySource::load(&example(MAIL_COLLECTION));
        let uid = account
            .uid()
            .expect("a source loaded from a file has a uid");

        for name in [MAIL_STORE, MAIL_IDENTITY, MAIL_TRANSPORT] {
            assert_eq!(
                RegistrySource::load(&example(name)).parent().as_deref(),
                Some(uid.as_str()),
                "docs/examples/{name} is parented to something else"
            );
        }
    });
}

#[test]
fn the_documented_mail_sources_point_at_each_other() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The chain `prepare_mail` describes and Evolution walks when the user hits
        // send: the receiving account names the identity, and the identity names the
        // transport that identity sends through. Written out by hand here, so the
        // two uids are two more strings that can be wrong.
        let store = RegistrySource::load(&example(MAIL_STORE));
        let identity = RegistrySource::load(&example(MAIL_IDENTITY));
        let transport = RegistrySource::load(&example(MAIL_TRANSPORT));

        // SAFETY: referencing the GTypes registers them, so the lookups below can
        // find the extensions; each is present on the source it is read from, so
        // nothing is created here.
        let (named_identity, named_transport) = unsafe {
            e_source_mail_account_get_type();
            e_source_mail_submission_get_type();
            let account: *mut ESourceMailAccount =
                e_source_get_extension(store.source, E_SOURCE_EXTENSION_MAIL_ACCOUNT.as_ptr())
                    .cast();
            let submission: *mut ESourceMailSubmission = e_source_get_extension(
                identity.source,
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
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Which of the three `child_added` writes a server onto, decided by the same
        // function the registry's callback reaches — so the `BackendName=jmap` lines
        // in two of these files are load-bearing and the identity's absence of one
        // is deliberate.
        for (name, extension) in [
            (MAIL_STORE, E_SOURCE_EXTENSION_MAIL_ACCOUNT),
            (MAIL_TRANSPORT, E_SOURCE_EXTENSION_MAIL_TRANSPORT),
        ] {
            let service = RegistrySource::load(&example(name));
            // SAFETY: a live source.
            assert_eq!(
                unsafe { mail_service_of(service.source) },
                Some(extension),
                "docs/examples/{name} is not a mail service of this account"
            );
        }

        let identity = RegistrySource::load(&example(MAIL_IDENTITY));
        // SAFETY: a live source.
        assert_eq!(unsafe { mail_service_of(identity.source) }, None);
    });
}

#[test]
fn the_documented_transport_reaches_the_mock_server() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // What the recipe's mail run is for, asserted at the far end: the transport
        // the reader writes carries a provider and no server at all, and what comes
        // back out of `~/.config/evolution/sources/` after the registry has seen it
        // is a source whose Camel settings name the mock. Read through `jmap-mail`'s
        // own reader, which is what `connect_sync` calls.
        let account = RegistrySource::load(&example(MAIL_COLLECTION));

        for name in [MAIL_STORE, MAIL_TRANSPORT] {
            let service = RegistrySource::load(&example(name));
            assert!(
                !service.has(E_SOURCE_EXTENSION_AUTHENTICATION),
                "docs/examples/{name} was written with a server of its own, so this \
                 test would pass without the binding it is checking"
            );

            // SAFETY: two live sources.
            unsafe { follow_collection(account.source, service.source) };

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
            // The mock speaks no TLS, and a mail source left claiming it does not
            // connect at all — the failure the recipe's reader would see first.
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

/// The contents of every ```` ```<language> ```` block in `markdown`.
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
