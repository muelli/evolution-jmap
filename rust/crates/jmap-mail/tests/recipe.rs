// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The mail provider's manual recipe, checked as far as a test can reach.
//
// `docs/manual-test-mail-provider.md` is the mail counterpart of the book,
// calendar, and collection recipes. It is prose. Nothing else in this
// repository fails when a group name in it is wrong. Nothing else fails
// when the protocol it tells the reader to write no longer matches the one
// that `camel_provider_module_init` registers. Nothing else fails when one
// of the two keyfiles that must share a server loses its copy.
//
// The symptom for a mail account is quieter than for an address book. There
// is no factory to report an error. Camel decides which module to dlopen by
// reading the `.urls` files in its provider directory. A `.source` file that
// names a protocol no file claims becomes an account that Evolution shows
// but cannot open. The failure appears as "no provider available for
// protocol" at connect time, not at load time. A transport with no server
// of its own is worse: the account receives mail without error and fails
// only when the user presses Send.
//
// So the keyfiles are real files: the ones in `docs/examples/` that the
// recipe tells the reader to copy. These tests read them the way the
// registry does. `e_server_side_source_new` on a GFile is exactly what
// `evolution-source-registry` calls for every file in its sources
// directory, and it needs neither a bus nor a daemon. What comes out is
// then read the way the provider reads it: `e_source_camel_get_settings`
// gets the `CamelSettings` that `e_source_camel_configure_service` would
// hand to a `CamelJmapStore`, and `ServerConfig::from_settings` (the call
// that `authenticate_sync` makes) reads them.
//
// This recipe differs from `jmap-backend-collection`'s `tests/recipe.rs` in
// one key way. There, the collection backend writes the account's server
// onto the mail sources, and the test asserts that binding. Here there is
// no collection and no binding. The reader writes the server twice instead,
// and the test asserts that both copies reach the mock.

use std::ffi::{CStr, c_char};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use eds_sys::{
    CAMEL_NETWORK_SECURITY_METHOD_NONE, CamelNetworkSecurityMethod, CamelSettings,
    E_SOURCE_EXTENSION_MAIL_ACCOUNT, E_SOURCE_EXTENSION_MAIL_SUBMISSION,
    E_SOURCE_EXTENSION_MAIL_TRANSPORT, ESource, ESourceBackend, ESourceCamel, ESourceMailAccount,
    ESourceMailSubmission, camel_network_settings_get_security_method,
    e_source_backend_get_backend_name, e_source_camel_generate_subtype,
    e_source_camel_get_extension_name, e_source_camel_get_settings, e_source_get_extension,
    e_source_get_parent, e_source_get_uid, e_source_has_extension,
    e_source_mail_account_get_identity_uid, e_source_mail_account_get_type,
    e_source_mail_submission_get_transport_uid, e_source_mail_submission_get_type,
    e_source_mail_transport_get_type,
};
use glib_sys::GFALSE;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::source::{ConnectTarget, RegistrySource, SourceError};
use jmap_mail::provider::PROTOCOL;
use jmap_mail::server::ServerConfig;
use jmap_mail::settings::settings_type;

/// Every keyfile that the recipe tells the reader to copy, in the order the
/// document quotes them: one `.source` file per ```` ```ini ```` block,
/// checked against the block below.
///
/// A mail account needs exactly three sources: the one Evolution receives
/// through, the one that names who the mail is from, and the one it sends
/// through. These names differ from the collection recipe's
/// `jmap-mock-mail-*` files on purpose. A file name is a source uid, so a
/// reader who has run both recipes keeps all seven files in one directory.
const KEYFILES: [&str; 3] = [
    "jmap-mock-standalone-mail.source",
    "jmap-mock-standalone-identity.source",
    "jmap-mock-standalone-transport.source",
];

const STORE: &str = KEYFILES[0];
const IDENTITY: &str = KEYFILES[1];
const TRANSPORT: &str = KEYFILES[2];

/// The repository root, from the crate this test lives in.
fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("the crate directory is inside the repository")
}

fn recipe() -> PathBuf {
    repository().join("docs/manual-test-mail-provider.md")
}

fn example(name: &str) -> PathBuf {
    repository().join("docs/examples").join(name)
}

/// The name `[JMAP Backend]`, the extension name that a `jmap` service's
/// Camel settings live under. This function first generates the
/// `ESourceCamel` subtype that carries the extension.
///
/// This helper matches `jmap-backend-collection`'s recipe test: same
/// reason, same `OnceLock`. Nothing here calls
/// `e_source_camel_register_types()`, because that call happens only when
/// Camel loads an installed provider, not when a test links one in. So this
/// function generates the one subtype these tests need directly. A race
/// between generating and registering the subtype aborts inside GObject,
/// and Rust runs the tests in this file as threads of one process.
fn camel_extension_name() -> *const c_char {
    static NAME: OnceLock<usize> = OnceLock::new();
    let name = *NAME.get_or_init(|| {
        // SAFETY: PROTOCOL is a NUL-terminated protocol name. settings_type
        // registers a GType derived from CamelSettings. The returned name is
        // interned and is never freed.
        unsafe {
            let gtype = e_source_camel_generate_subtype(PROTOCOL.as_ptr(), settings_type());
            assert_ne!(
                gtype, 0,
                "no ESourceCamel subtype was generated for the jmap protocol"
            );
            e_source_camel_get_extension_name(PROTOCOL.as_ptr()) as usize
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

    /// The uid that the registry gives this file: its file name without the
    /// extension. Another file's `IdentityUid=` or `TransportUid=` must
    /// spell this same string.
    fn uid(&self) -> Option<String> {
        // SAFETY: a live source; the getter returns a string owned by it.
        unsafe { read_string(e_source_get_uid(self.as_ptr())) }
    }

    /// The parent that this source names. For all three of these sources,
    /// it is nothing.
    fn parent(&self) -> Option<String> {
        // SAFETY: as above.
        unsafe { read_string(e_source_get_parent(self.as_ptr())) }
    }

    fn has(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a header constant.
        unsafe { e_source_has_extension(self.as_ptr(), name.as_ptr()) != GFALSE }
    }

    /// The backend name that the named `ESourceBackend`-derived extension
    /// reports, or nothing if this source has no such extension.
    ///
    /// This method never creates the extension on demand.
    /// `e_source_get_extension` would create it, and a created-on-demand
    /// `[Mail Transport]` on the identity would make every file answer yes
    /// to "is this a transport".
    fn backend_name(&self, extension_name: &CStr) -> Option<String> {
        if !self.has(extension_name) {
            return None;
        }
        // SAFETY: the source is known to have this extension, fetched by
        // its header constant. The getter returns a string owned by the
        // extension.
        let name = unsafe {
            let extension = e_source_get_extension(self.as_ptr(), extension_name.as_ptr())
                .cast::<ESourceBackend>();
            e_source_backend_get_backend_name(extension)
        };
        // SAFETY: a non-NULL return is a NUL-terminated string owned by the
        // extension. The extension outlives the copy made here.
        (!name.is_null()).then(|| {
            unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned()
        })
    }

    /// The `CamelSettings` object that a Camel service configured from this
    /// source would receive. This method performs
    /// `e_source_camel_configure_service`'s own two steps, without the
    /// service.
    fn camel_settings(&self) -> *mut CamelSettings {
        // SAFETY: the source is live, and the extension name is the
        // interned name of a registered `ESourceCamel` subtype. The source
        // creates the extension on demand and owns it, and it owns the
        // settings object the extension holds.
        unsafe {
            let extension: *mut ESourceCamel =
                e_source_get_extension(self.as_ptr(), camel_extension_name()).cast();
            assert!(!extension.is_null(), "the jmap subtype is not registered");
            let settings = e_source_camel_get_settings(extension);
            assert!(!settings.is_null(), "the extension holds no settings");
            settings
        }
    }

    /// The `CamelNetworkSecurityMethod` that these settings produce. This is
    /// the value that the `[Security] Method` string converts into, not the
    /// string itself. A wrong spelling fails silently during this
    /// conversion.
    fn camel_security_method(&self) -> CamelNetworkSecurityMethod {
        // SAFETY: the settings object of a live source, which implements
        // `CamelNetworkSettings`.
        unsafe { camel_network_settings_get_security_method(self.camel_settings().cast()) }
    }

    /// The server that the provider reads from these settings. This is the
    /// last value that `authenticate_sync` needs before it builds a client.
    fn server(&self) -> Result<ServerConfig, SourceError> {
        // SAFETY: the settings object of a live source, only read from.
        unsafe { ServerConfig::from_settings(self.camel_settings()) }
    }
}

#[test]
fn the_recipe_quotes_every_keyfile_verbatim() {
    // The reader copies the files but reads the document. If the two drift
    // apart, the reader trusts the wrong one. So an `ini` block in this
    // document must quote a whole keyfile verbatim, and nothing else.
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
            "docs/manual-test-mail-provider.md and docs/examples/{name} disagree"
        );
    }
}

#[test]
fn the_documented_services_name_the_protocol_the_provider_registers() {
    // Camel keys its provider table by this string. It reads the string
    // from the `.urls` file beside each installed module to decide what to
    // dlopen, and it reads the same string from a `.source` file's
    // `BackendName`. A typo in either keyfile creates an account that
    // appears in Evolution and cannot be opened.
    let protocol = PROTOCOL.to_str().expect("the protocol is UTF-8");

    // SAFETY: referencing the GTypes registers them, so the extension
    // lookups below can find them. This binary has not otherwise touched a
    // mail source.
    unsafe {
        e_source_mail_account_get_type();
        e_source_mail_transport_get_type();
    }

    for (name, extension) in [
        (STORE, E_SOURCE_EXTENSION_MAIL_ACCOUNT),
        (TRANSPORT, E_SOURCE_EXTENSION_MAIL_TRANSPORT),
    ] {
        assert_eq!(
            Example::load(&example(name))
                .backend_name(extension)
                .as_deref(),
            Some(protocol),
            "docs/examples/{name} names a provider Camel would not route here"
        );
    }

    // The identity is neither a store nor a transport. It names a person,
    // not a service. It reaches no server, and nothing reads a
    // `BackendName` on it.
    let identity = Example::load(&example(IDENTITY));
    assert_eq!(identity.backend_name(E_SOURCE_EXTENSION_MAIL_ACCOUNT), None);
    assert_eq!(
        identity.backend_name(E_SOURCE_EXTENSION_MAIL_TRANSPORT),
        None
    );
}

#[test]
fn the_documented_sources_stand_on_their_own() {
    // This differs from the collection recipe in substance, not only style.
    // EDS does not export a source whose `Parent=` names a collection that
    // does not exist. A leftover parent line copied from another file would
    // make the registry drop all three sources on load. The account would
    // not appear in Evolution, with no error reported. There is no account
    // for these sources to hang off until M7's setup UI creates one.
    for name in KEYFILES {
        assert_eq!(
            Example::load(&example(name)).parent(),
            None,
            "docs/examples/{name} hangs off a collection this recipe does not write"
        );
    }
}

#[test]
fn the_documented_sources_point_at_each_other() {
    // This is the chain that Evolution walks when the user sends mail. The
    // receiving account names the identity, and the identity names the
    // transport it sends through. Both uids are written by hand, so both
    // can be wrong. The failure then appears only at the first Send, not
    // at the first open.
    let store = Example::load(&example(STORE));
    let identity = Example::load(&example(IDENTITY));
    let transport = Example::load(&example(TRANSPORT));

    // SAFETY: referencing the GTypes registers them, so the lookups below
    // can find the extensions. Each extension is present on the source it
    // is read from, so this code creates nothing here.
    let (named_identity, named_transport) = unsafe {
        e_source_mail_account_get_type();
        e_source_mail_submission_get_type();
        let account: *mut ESourceMailAccount =
            e_source_get_extension(store.as_ptr(), E_SOURCE_EXTENSION_MAIL_ACCOUNT.as_ptr()).cast();
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
}

#[test]
fn the_documented_services_both_reach_the_mock_server() {
    // This test checks the end goal of this recipe: reading through the
    // provider's own reader, the one that `connect_sync` calls, from the
    // `CamelSettings` that `e_source_camel_configure_service` would hand
    // the store.
    //
    // The test checks both files, and that is the point. A collection
    // account writes its server onto its mail sources. A hand-written
    // account has no collection to do that, so the `[Authentication]`
    // group appears twice. A transport that lost its copy becomes an
    // account that receives mail but cannot send it.
    for name in [STORE, TRANSPORT] {
        let service = Example::load(&example(name));
        assert_eq!(
            service.server(),
            Ok(ServerConfig {
                target: ConnectTarget::Origin("http://127.0.0.1:8080".to_owned()),
                user: None,
            }),
            "docs/examples/{name} does not reach the mock server"
        );
        // `jmap-mockd` speaks no TLS. A service that still claims TLS does
        // not connect at all. This is the first failure the recipe's
        // reader would see.
        //
        // The keyfile writes `Method=none`. This is the only spelling that
        // passes the client's refusal to send credentials in the clear, and
        // it passes only because the host is loopback.
        assert_eq!(
            service.camel_security_method(),
            CAMEL_NETWORK_SECURITY_METHOD_NONE,
            "docs/examples/{name} claims a security method the mock does not speak"
        );
    }
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
