// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The mail provider's manual recipe, checked as far as a test can reach.
//
// `docs/manual-test-mail-provider.md` is the mail counterpart of the book,
// calendar and collection recipes, and it is prose: nothing else in this
// repository fails when a group name in it is wrong, when the protocol it tells
// the reader to write stops being the one `camel_provider_module_init`
// registers, or when the two keyfiles that need a server between them lose one.
//
// The symptom for a mail account is quieter than for an address book. There is
// no factory to complain: Camel decides which module to dlopen by reading the
// `.urls` files in its provider directory, so a `.source` naming a protocol no
// file claims is an account Evolution shows and cannot open, with the failure
// arriving as "no provider available for protocol" at connect time rather than
// as anything at load time. And a transport with no server of its own is worse
// again — the account receives mail perfectly and fails only when the user
// presses Send.
//
// So the keyfiles are files, the ones in `docs/examples/` the recipe says to
// copy, and these tests read them the way the registry does.
// `e_server_side_source_new` on a GFile is exactly what
// `evolution-source-registry` calls for every file in its sources directory and
// needs neither a bus nor a daemon. What comes out is then read the way the
// provider reads it: `e_source_camel_get_settings` for the `CamelSettings` an
// `e_source_camel_configure_service` would hand a `CamelJmapStore`, and
// `ServerConfig::from_settings` — the call `connect_sync` makes — on top of it.
//
// The difference from `jmap-backend-collection`'s `tests/recipe.rs` is the
// whole point of this recipe: there, the account's server is written onto the
// mail sources by the collection backend, and the test asserts the binding.
// Here there is no collection and no binding, so the reader writes the server
// twice, and what is asserted is that both copies of it reach the mock.

use std::ffi::{CStr, c_char};
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::OnceLock;

use eds_sys::{
    CAMEL_NETWORK_SECURITY_METHOD_NONE, CamelNetworkSecurityMethod, CamelSettings,
    E_SOURCE_EXTENSION_MAIL_ACCOUNT, E_SOURCE_EXTENSION_MAIL_SUBMISSION,
    E_SOURCE_EXTENSION_MAIL_TRANSPORT, ESource, ESourceBackend, ESourceCamel, ESourceMailAccount,
    ESourceMailSubmission, ESourceRegistryServer, camel_network_settings_get_security_method,
    e_server_side_source_new, e_source_backend_get_backend_name, e_source_camel_generate_subtype,
    e_source_camel_get_extension_name, e_source_camel_get_settings, e_source_get_extension,
    e_source_get_parent, e_source_get_uid, e_source_has_extension,
    e_source_mail_account_get_identity_uid, e_source_mail_account_get_type,
    e_source_mail_submission_get_transport_uid, e_source_mail_submission_get_type,
    e_source_mail_transport_get_type, e_source_registry_server_new, g_file_new_for_path,
    g_object_unref,
};
use glib_sys::GFALSE;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::source::{ConnectTarget, SourceError};
use jmap_mail::provider::PROTOCOL;
use jmap_mail::server::ServerConfig;
use jmap_mail::settings::settings_type;

/// Every keyfile the recipe tells the reader to copy, in the order the document
/// quotes them — one `.source` file per ```` ```ini ```` block, checked against
/// the block below.
///
/// The three sources a mail account is made of and nothing else: what Evolution
/// receives through, who the mail is from, and what it is sent through. Named
/// apart from the collection recipe's `jmap-mock-mail-*` files on purpose — a
/// file name is a source uid, and a reader who has run both recipes has all
/// seven in one directory.
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

/// The name `[JMAP Backend]` — the extension a `jmap` service's Camel settings
/// live under — having first generated the `ESourceCamel` subtype that carries
/// it.
///
/// The same helper, for the same reason and with the same `OnceLock`, as
/// `jmap-backend-collection`'s recipe test: nothing here has called
/// `e_source_camel_register_types()`, because that happens when Camel loads an
/// installed provider rather than when a test links one in, so the one subtype
/// these tests need is generated directly. Losing the race between generating
/// and registering it is an abort inside GObject, and Rust runs the tests in
/// this file as threads of one process.
fn camel_extension_name() -> *const c_char {
    static NAME: OnceLock<usize> = OnceLock::new();
    let name = *NAME.get_or_init(|| {
        // SAFETY: a NUL-terminated protocol name and a GType derived from
        // CamelSettings, which is what `settings_type` registers; the name it
        // hands back is interned and never freed.
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

/// A `.source` file loaded the way `evolution-source-registry` loads one.
///
/// The fourth copy of the same stand-in — the book, calendar and collection
/// recipe tests each have one — and a fourth copy rather than a shared helper
/// for the reason the third one gives: cargo builds each integration test as
/// its own binary, and the alternative is a workspace crate that exists to hold
/// thirty lines.
struct RegistrySource {
    server: *mut gobject_sys::GObject,
    source: *mut ESource,
}

impl RegistrySource {
    /// Parses `path`.
    ///
    /// `e_server_side_source_new` insists on a registry server — it is the
    /// object a server-side source reports changes to — but constructing one
    /// neither owns a bus name nor reads the user's sources; that only happens
    /// when it is run.
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
    /// extension — and so the string another file's `IdentityUid=` or
    /// `TransportUid=` has to spell.
    fn uid(&self) -> Option<String> {
        // SAFETY: a live source; the getter returns a string owned by it.
        unsafe { read_string(e_source_get_uid(self.source)) }
    }

    /// What this source says it hangs off, which for all three of these is
    /// nothing.
    fn parent(&self) -> Option<String> {
        // SAFETY: as above.
        unsafe { read_string(e_source_get_parent(self.source)) }
    }

    fn has(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a header constant.
        unsafe { e_source_has_extension(self.source, name.as_ptr()) != GFALSE }
    }

    /// What the named `ESourceBackend`-derived extension says the backend is
    /// called, or nothing if this source has no such extension.
    ///
    /// Deliberately does not create one: `e_source_get_extension` would, and a
    /// created-on-demand `[Mail Transport]` on the identity would make the
    /// question "is this a transport" answer yes for every file.
    fn backend_name(&self, extension_name: &CStr) -> Option<String> {
        if !self.has(extension_name) {
            return None;
        }
        // SAFETY: an extension the source is known to have, fetched by its
        // header constant; the getter returns a string owned by it.
        let name = unsafe {
            let extension = e_source_get_extension(self.source, extension_name.as_ptr())
                .cast::<ESourceBackend>();
            e_source_backend_get_backend_name(extension)
        };
        // SAFETY: a non-NULL return is a NUL-terminated string owned by the
        // extension, which outlives the copy taken here.
        (!name.is_null()).then(|| {
            unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned()
        })
    }

    /// The `CamelSettings` object a Camel service configured from this source
    /// would be given — `e_source_camel_configure_service`'s own two steps with
    /// the service left out.
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

    /// The server the provider reads off those settings — the last thing
    /// `connect_sync` needs before it builds a client.
    fn server(&self) -> Result<ServerConfig, SourceError> {
        // SAFETY: the settings object of a live source, only read from.
        unsafe { ServerConfig::from_settings(self.camel_settings()) }
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
fn the_recipe_quotes_every_keyfile_verbatim() {
    // The reader copies the files but reads the document; if the two drift
    // apart, whichever one they trusted is the wrong one. So an `ini` block in
    // this document is a whole keyfile quoted verbatim and nothing else.
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
    // Camel keys its provider table by this string, reads it out of the `.urls`
    // file beside each installed module to decide what to dlopen, and gets it
    // from a `.source` file's `BackendName`. A typo in either keyfile is an
    // account that appears in Evolution and cannot be opened.
    let protocol = PROTOCOL.to_str().expect("the protocol is UTF-8");

    // SAFETY: referencing the GTypes registers them, so that the extension
    // lookups below can find them in a binary that has not otherwise touched a
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
            RegistrySource::load(&example(name))
                .backend_name(extension)
                .as_deref(),
            Some(protocol),
            "docs/examples/{name} names a provider Camel would not route here"
        );
    }

    // And the identity is neither: it is a person rather than a service, it
    // reaches no server, and a `BackendName` on it would be read by nothing.
    let identity = RegistrySource::load(&example(IDENTITY));
    assert_eq!(identity.backend_name(E_SOURCE_EXTENSION_MAIL_ACCOUNT), None);
    assert_eq!(
        identity.backend_name(E_SOURCE_EXTENSION_MAIL_TRANSPORT),
        None
    );
}

#[test]
fn the_documented_sources_stand_on_their_own() {
    // The difference from the collection recipe, and not a stylistic one: EDS
    // does not export a source whose `Parent=` names a collection that is not
    // there, so a leftover parent line from a copied file is three sources the
    // registry drops on load — no account in Evolution, and nothing said about
    // it. There is no account to hang these off until M7's setup UI writes one.
    for name in KEYFILES {
        assert_eq!(
            RegistrySource::load(&example(name)).parent(),
            None,
            "docs/examples/{name} hangs off a collection this recipe does not write"
        );
    }
}

#[test]
fn the_documented_sources_point_at_each_other() {
    // The chain Evolution walks when the user hits send: the receiving account
    // names the identity, and the identity names the transport it sends
    // through. Two uids written by hand, so two more strings that can be wrong
    // — and the failure is deferred to the first Send rather than seen on the
    // first open.
    let store = RegistrySource::load(&example(STORE));
    let identity = RegistrySource::load(&example(IDENTITY));
    let transport = RegistrySource::load(&example(TRANSPORT));

    // SAFETY: referencing the GTypes registers them, so the lookups below can
    // find the extensions; each is present on the source it is read from, so
    // nothing is created here.
    let (named_identity, named_transport) = unsafe {
        e_source_mail_account_get_type();
        e_source_mail_submission_get_type();
        let account: *mut ESourceMailAccount =
            e_source_get_extension(store.source, E_SOURCE_EXTENSION_MAIL_ACCOUNT.as_ptr()).cast();
        let submission: *mut ESourceMailSubmission =
            e_source_get_extension(identity.source, E_SOURCE_EXTENSION_MAIL_SUBMISSION.as_ptr())
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
    // What this recipe is for, at the far end: read through the provider's own
    // reader, which is what `connect_sync` calls, off the `CamelSettings` an
    // `e_source_camel_configure_service` would hand the store.
    //
    // Both files, and that is the point. A collection account writes its server
    // onto its mail sources; a hand-written account has nobody to do that, so
    // the `[Authentication]` group appears twice, and a transport that lost its
    // copy is an account that receives mail and cannot send it.
    for name in [STORE, TRANSPORT] {
        let service = RegistrySource::load(&example(name));
        assert_eq!(
            service.server(),
            Ok(ServerConfig {
                target: ConnectTarget::Origin("http://127.0.0.1:8080".to_owned()),
                user: None,
            }),
            "docs/examples/{name} does not reach the mock server"
        );
        // `jmap-mockd` speaks no TLS, and a service left claiming it does not
        // connect at all — the failure the recipe's reader would see first.
        // Written as `Method=none`, which is also the only spelling that gets
        // past the client's refusal to send credentials in the clear, and then
        // only because the host is loopback.
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
