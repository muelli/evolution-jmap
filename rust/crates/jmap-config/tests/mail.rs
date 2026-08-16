// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The three mail sources a setup commits beside the account, against real
// `ESource`s — and held against the collection backend's `prepare_mail`, which
// is the registry-side writer of the same three sources.
//
// The join is again the point, as in `tests/account.rs`, but it runs the other
// way round. There the reader was somebody else's and this crate's writer had
// to agree with it; here there are two *writers* — this one, which runs in
// Evolution's process where the user's answers are, and the vfunc, which runs
// in `evolution-source-registry` where the factory is. Two writers of one file
// that are only checked separately are two writers that drift, and the drift is
// silent: an account whose mail sources name a protocol Camel has no provider
// for is an inbox that never opens, with nothing in any log to say which of the
// two wrote the name.
//
// And then there is a reader after all, one source further down than the account
// tests could reach: the `CamelSettings` object an `ESourceCamel` extension hands
// a `CamelJmapStore`. It is not this crate's, it is `jmap-mail`'s, and it is the
// only place to ask an account the question the provider will ask it — because
// none of the four fields that answer it is stored where it appears to be. Host,
// port, user and encryption are bound out of `[Authentication]` and `[Security]`
// into a settings object by machinery neither writer mentions, over a conversion
// that turns one of them into a different vocabulary and silently keeps its own
// default when it cannot. So these tests generate the subtype, ask the extension
// for its settings, and read them with the provider's own `ServerConfig`.

use std::ffi::{CStr, CString, c_char};
use std::ptr;
use std::sync::OnceLock;

use eds_sys::{
    CAMEL_NETWORK_SECURITY_METHOD_NONE, CAMEL_NETWORK_SECURITY_METHOD_SSL_ON_ALTERNATE_PORT,
    CamelNetworkSecurityMethod, CamelSettings, E_SOURCE_EXTENSION_AUTHENTICATION,
    E_SOURCE_EXTENSION_MAIL_ACCOUNT, E_SOURCE_EXTENSION_MAIL_IDENTITY,
    E_SOURCE_EXTENSION_MAIL_SUBMISSION, E_SOURCE_EXTENSION_MAIL_TRANSPORT,
    E_SOURCE_EXTENSION_SECURITY, ESource, ESourceBackend, ESourceCamel, ESourceMailAccount,
    ESourceMailIdentity, ESourceMailSubmission, ESourceSecurity,
    camel_network_settings_get_security_method, e_collection_backend_factory_prepare_mail,
    e_source_backend_get_backend_name, e_source_camel_generate_subtype,
    e_source_camel_get_extension_name, e_source_camel_get_settings,
    e_source_collection_get_identity, e_source_get_extension, e_source_get_parent,
    e_source_get_uid, e_source_has_extension, e_source_mail_account_get_identity_uid,
    e_source_mail_identity_get_address, e_source_mail_submission_get_transport_uid, e_source_new,
    e_source_new_with_uid, e_source_security_get_secure,
    e_util_can_use_collection_as_credential_source,
};
use glib_sys::GFALSE;
use gobject_sys::{g_object_unref, g_type_create_instance};
use jmap_backend_collection::factory::JmapCollectionFactory;
use jmap_backend_collection::mail_child;
use jmap_backend_collection::resource_id::resource_id_of;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::source::SourceError;
use jmap_backend_core::subclass::register_static;
use jmap_collection_sync::Parts;
use jmap_collection_sync::child_source::Connection;
use jmap_config::account::{Account, apply as apply_account};
use jmap_config::mail::{
    MAIL_BACKEND_NAME, MAIL_SECURITY_METHOD_NONE, MAIL_SECURITY_METHOD_TLS, MailSources, apply,
};
use jmap_mail::server::ServerConfig;
use jmap_mail::settings::settings_type;

/// The uid the account source is created with, so that the parent the three
/// mail sources carry is a string this file can name rather than one it read
/// off the very source it is checking.
const ACCOUNT_UID: &str = "jmap-account";

/// The name `[JMAP Backend]` — the extension a `jmap` service's Camel settings
/// live under — having first generated the `ESourceCamel` subtype that carries
/// it.
///
/// `e_source_camel_register_types()` is what normally does this, by loading
/// every installed Camel provider module and generating one subtype per service
/// class it finds. Here the provider is linked in rather than installed, so the
/// one subtype these tests need is generated directly, which is the case
/// `e_source_camel_generate_subtype` documents as its own reason to be public
/// API. It is also what M7's module will have to do, for the same reason
/// Evolution's own account editor does not have to: nothing has loaded
/// `libcameljmap.so` at the point the setup writes an account.
///
/// The `OnceLock` is not decoration. `generate_subtype` reads the type name and
/// then registers it, which is two steps, and Rust runs the tests in this file
/// as threads of one process; losing that race is an abort inside GObject. The
/// name is kept as a `usize` because a raw pointer is not `Sync` — the string
/// itself is `g_intern_string`'d and so lives as long as the process.
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

/// An `ESource` and the one reference to it.
struct Source(*mut ESource);

impl Source {
    /// A blank source with a minted uid — the state the setup's scratch mail
    /// sources are in. `e_source_new (NULL, NULL, &error)` is what EDS itself
    /// uses for a source with no keyfile behind it yet.
    fn blank() -> Self {
        let mut error = ptr::null_mut();
        // SAFETY: no related object, no file, and a GError out-parameter are
        // the documented arguments.
        let source = unsafe { e_source_new(ptr::null_mut(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new failed");
        Self(source)
    }

    fn with_uid(uid: &str) -> Self {
        let uid = CString::new(uid).expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a GError
        // out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    fn uid(&self) -> Option<String> {
        // SAFETY: a live source; the uid it returns is owned by it.
        unsafe { read_string(e_source_get_uid(self.0)) }
    }

    fn parent(&self) -> Option<String> {
        // SAFETY: as above, for the other string on `[Data Source]`.
        unsafe { read_string(e_source_get_parent(self.0)) }
    }

    fn has_extension(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a NUL-terminated header constant.
        unsafe { e_source_has_extension(self.0, name.as_ptr()) != GFALSE }
    }

    /// The `ESourceBackend:backend-name` under `name`, asked for only once the
    /// extension is known to be there: `e_source_get_extension` creates what it
    /// cannot find, so a test that read through it without checking first would
    /// turn "the writer wrote nothing" into "the writer wrote NULL".
    fn backend_name(&self, name: &CStr) -> Option<String> {
        assert!(
            self.has_extension(name),
            "the source has no {name:?} to read a backend name from"
        );
        // SAFETY: the extension is present, so this returns the source's own,
        // owned by it; the name it holds is NULL or a NUL-terminated string
        // with the same lifetime.
        unsafe {
            let backend: *mut ESourceBackend = e_source_get_extension(self.0, name.as_ptr()).cast();
            read_string(e_source_backend_get_backend_name(backend))
        }
    }

    /// `[Mail Account] IdentityUid` — which identity this receiving account
    /// sends as.
    fn identity_uid(&self) -> Option<String> {
        assert!(
            self.has_extension(E_SOURCE_EXTENSION_MAIL_ACCOUNT),
            "the source is not a mail account"
        );
        // SAFETY: the extension is present; the uid it holds is NULL or a
        // NUL-terminated string owned by it.
        unsafe {
            let account: *mut ESourceMailAccount =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_MAIL_ACCOUNT.as_ptr()).cast();
            read_string(e_source_mail_account_get_identity_uid(account))
        }
    }

    /// `[Mail Submission] TransportUid` — which service this identity's mail
    /// leaves through.
    fn transport_uid(&self) -> Option<String> {
        assert!(
            self.has_extension(E_SOURCE_EXTENSION_MAIL_SUBMISSION),
            "the source has no submission settings"
        );
        // SAFETY: as above, with `[Mail Submission]` the extension.
        unsafe {
            let submission: *mut ESourceMailSubmission =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_MAIL_SUBMISSION.as_ptr()).cast();
            read_string(e_source_mail_submission_get_transport_uid(submission))
        }
    }

    /// `[Mail Identity] Address` — the address sent mail says it is from.
    fn address(&self) -> Option<String> {
        assert!(
            self.has_extension(E_SOURCE_EXTENSION_MAIL_IDENTITY),
            "the source is not a mail identity"
        );
        // SAFETY: as above, with `[Mail Identity]` the extension.
        unsafe {
            let identity: *mut ESourceMailIdentity =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_MAIL_IDENTITY.as_ptr()).cast();
            read_string(e_source_mail_identity_get_address(identity))
        }
    }

    /// The `CamelSettings` object a Camel service configured from this source
    /// would be given.
    ///
    /// These are `e_source_camel_configure_service`'s own two steps with the
    /// service left out — ask the source for the provider's extension, ask the
    /// extension for its settings — so what comes back is the very object a
    /// `CamelJmapStore` is handed, host, port, user and all. None of those four
    /// is stored *in* the extension: `ESourceCamel` binds them bidirectionally
    /// to `[Authentication]` and `[Security]` on this same source, which is why
    /// the setup writes those groups and why this is the only reader that proves
    /// it did.
    fn camel_settings(&self) -> *mut CamelSettings {
        // SAFETY: a live source and the interned extension name of a
        // registered `ESourceCamel` subtype; the extension is created on demand
        // and owned by the source, and so is the settings object it holds.
        unsafe {
            let extension: *mut ESourceCamel =
                e_source_get_extension(self.0, camel_extension_name()).cast();
            assert!(
                !extension.is_null(),
                "the jmap ESourceCamel subtype is not registered, so no source \
                 has Camel settings to read"
            );
            let settings = e_source_camel_get_settings(extension);
            assert!(!settings.is_null(), "the extension holds no settings");
            settings
        }
    }

    /// The `CamelNetworkSecurityMethod` those settings arrived at — the value
    /// the string in `[Security] Method` was converted into, rather than the
    /// string itself, because the conversion is where a wrong spelling goes
    /// quiet.
    fn security_method(&self) -> CamelNetworkSecurityMethod {
        // SAFETY: the settings object of a live source. It implements
        // `CamelNetworkSettings` — `CamelJmapSettings` claims the interface, and
        // an `ESourceCamel` subtype generated from a settings class that did not
        // would have nothing to bind.
        unsafe { camel_network_settings_get_security_method(self.camel_settings().cast()) }
    }

    /// The server `jmap-mail` reads off those settings — the last thing its
    /// `connect_sync` needs before it builds a client, and therefore the whole
    /// question this source is written to answer.
    fn server(&self) -> Result<ServerConfig, SourceError> {
        // SAFETY: the settings object of a live source, only read from.
        unsafe { ServerConfig::from_settings(self.camel_settings()) }
    }

    /// `[Security] Secure` — the boolean EDS derives from the method string,
    /// and what every non-mail reader of this account asks.
    fn secure(&self) -> bool {
        assert!(
            self.has_extension(E_SOURCE_EXTENSION_SECURITY),
            "the source says nothing about transport security"
        );
        // SAFETY: the extension is present, so this returns the source's own.
        unsafe {
            let security: *mut ESourceSecurity =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_SECURITY.as_ptr()).cast();
            e_source_security_get_secure(security) != GFALSE
        }
    }

    /// `[Collection] Identity`, off the account source — the other place one
    /// account writes an address, and the one the two have to agree on.
    fn collection_identity(&self) -> Option<String> {
        // SAFETY: `apply_account` created the extension, so this returns the
        // source's own.
        unsafe {
            read_string(e_source_collection_get_identity(
                e_source_get_extension(self.0, eds_sys::E_SOURCE_EXTENSION_COLLECTION.as_ptr())
                    .cast(),
            ))
        }
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: this holds the only reference.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// An account and the four sources it is made of, after the setup has written
/// all of them.
struct Committed {
    collection: Source,
    account: Source,
    identity: Source,
    transport: Source,
}

impl Committed {
    fn new(account: &Account) -> Self {
        let committed = Self {
            collection: Source::with_uid(ACCOUNT_UID),
            account: Source::blank(),
            identity: Source::blank(),
            transport: Source::blank(),
        };
        committed.written(account)
    }

    /// What a commit does, in the order it does it: the account source first,
    /// then the three that hang off it.
    fn written(self, account: &Account) -> Self {
        // SAFETY: four live sources, none of which this call keeps.
        unsafe {
            apply_account(self.collection.0, account);
            apply(self.collection.0, &self.sources(), account);
        }
        self
    }

    fn sources(&self) -> MailSources {
        MailSources {
            account: self.account.0,
            identity: self.identity.0,
            transport: self.transport.0,
        }
    }
}

/// The same three sources as the registry would leave them: blank ones run
/// through the collection factory's `prepare_mail` vfunc, reached the way EDS
/// reaches it.
struct Prepared {
    account: Source,
    identity: Source,
    transport: Source,
}

impl Prepared {
    fn new() -> Self {
        // Registered statically rather than through the module entry point, as
        // in `jmap-backend-collection`'s own tests: there is no module here for
        // the type to belong to, and `register_static` hands an
        // already-registered type straight back.
        let gtype = register_static::<JmapCollectionFactory>();
        assert_ne!(gtype, 0, "the factory type is not registered");
        // `g_type_create_instance` rather than `g_object_new`, because
        // `EExtension:extensible` is CONSTRUCT_ONLY and a property-less
        // `g_object_new` earns a critical that has nothing to do with this test.
        // SAFETY: a registered, instantiatable type.
        let factory = unsafe { g_type_create_instance(gtype) };
        assert!(!factory.is_null(), "g_type_create_instance returned NULL");

        let prepared = Self {
            account: Source::blank(),
            identity: Source::blank(),
            transport: Source::blank(),
        };
        // SAFETY: a live factory of a type derived from
        // ECollectionBackendFactory and three live sources; the call keeps
        // none of them.
        unsafe {
            e_collection_backend_factory_prepare_mail(
                factory.cast(),
                prepared.account.0,
                prepared.identity.0,
                prepared.transport.0,
            );
        }
        // SAFETY: the reference instance creation left behind, given back once.
        unsafe { g_object_unref(factory.cast()) };

        prepared
    }
}

/// An account with every field filled in, so that a string written onto the
/// wrong source has somewhere wrong to land.
fn account() -> Account {
    Account {
        identity: "vera@example.com".to_owned(),
        connection: Connection {
            host: "jmap.example.com".to_owned(),
            port: Some(8443),
            user: Some("vera".to_owned()),
            auth_method: Some("plain/password".to_owned()),
            secure: true,
        },
        parts: Parts::ALL,
        oauth2_registered: false,
    }
}

/// What makes the three sources *this account's* mail rather than three loose
/// sources in the registry's directory.
///
/// `e_collection_backend_list_mail_sources()` finds them by walking the
/// account's children, and `collection_backend_bind_child_enabled()` binds each
/// one's `enabled` to the account's `mail-enabled` on the same walk. An
/// unparented mail source is not a broken account: it is a second, top-level
/// account in the sidebar that the collection knows nothing about, and that no
/// "receive mail for this account" switch reaches.
#[test]
fn the_three_mail_sources_hang_off_the_account() {
    let committed = Committed::new(&account());

    for (what, source) in [
        ("the mail account", &committed.account),
        ("the identity", &committed.identity),
        ("the transport", &committed.transport),
    ] {
        assert_eq!(
            source.parent().as_deref(),
            Some(ACCOUNT_UID),
            "{what} is not parented to the account"
        );
    }
    assert_eq!(committed.collection.uid().as_deref(), Some(ACCOUNT_UID));
}

/// The mail account is what Evolution receives through, and its backend name is
/// the Camel protocol it receives through — `libcameljmap.so`'s.
///
/// A mail account source without one is not an error anywhere: it is an account
/// Evolution shows and cannot open, because `camel_provider_get` is asked for
/// the empty protocol.
#[test]
fn the_mail_account_is_served_by_the_jmap_camel_provider() {
    let committed = Committed::new(&account());

    assert_eq!(
        committed
            .account
            .backend_name(E_SOURCE_EXTENSION_MAIL_ACCOUNT)
            .as_deref(),
        MAIL_BACKEND_NAME.to_str().ok(),
    );
}

/// And so is the transport, under the *same* name: JMAP submits over the
/// session it reads through, so `jmap-mail`'s provider registers one protocol
/// with both a store and a transport type in it, and there is no second service
/// beside it the way `smtp` sits beside `imapx`.
#[test]
fn the_transport_is_the_same_provider_rather_than_a_second_one() {
    let committed = Committed::new(&account());

    assert_eq!(
        committed
            .transport
            .backend_name(E_SOURCE_EXTENSION_MAIL_TRANSPORT)
            .as_deref(),
        MAIL_BACKEND_NAME.to_str().ok(),
    );
}

/// The two links that make three sources one account: the account names the
/// identity it sends as, the identity names the transport its mail leaves
/// through.
///
/// Both are uids, and both are read back off the very sources they should name
/// — a link written to the wrong uid is not a failure at commit time but a
/// `From:` header from some other account, or a send that picks some other
/// account's server.
#[test]
fn the_account_names_the_identity_and_the_identity_names_the_transport() {
    let committed = Committed::new(&account());

    assert_eq!(
        committed.account.identity_uid(),
        committed.identity.uid(),
        "the mail account does not name the identity it was committed with"
    );
    assert_eq!(
        committed.identity.transport_uid(),
        committed.transport.uid(),
        "the identity does not name the transport it was committed with"
    );
}

/// The address the identity sends from is the address the account says it is,
/// because they are the same answer to the same question and the user typed it
/// once.
///
/// `[Collection] Identity` and `[Mail Identity] Address` living in two
/// different sources is EDS's arrangement, not a choice available here; what is
/// a choice is whether both are written from the one string, and a setup that
/// let them disagree would send mail from an address the account does not
/// claim.
#[test]
fn the_address_mail_is_sent_from_is_the_identity_the_account_claims() {
    let committed = Committed::new(&account());

    assert_eq!(
        committed.identity.address().as_deref(),
        Some("vera@example.com")
    );
    assert_eq!(
        committed.identity.address(),
        committed.collection.collection_identity(),
    );
}

/// The identity is a person, not a service, and giving it a service extension
/// would make it one.
///
/// `collection_backend_child_is_mail()` treats a source carrying any of the
/// three mail extensions as mail, so an identity that also carried
/// `[Mail Account]` would be a second, empty receiving account of this user's —
/// visible in Evolution, pointed at nothing.
#[test]
fn the_identity_is_not_turned_into_a_service_of_its_own() {
    let committed = Committed::new(&account());

    for extension in [
        E_SOURCE_EXTENSION_MAIL_ACCOUNT,
        E_SOURCE_EXTENSION_MAIL_TRANSPORT,
    ] {
        assert!(
            !committed.identity.has_extension(extension),
            "the identity was given {extension:?}, which makes it a mail \
             service in its own right"
        );
    }
}

/// And the account and the transport are services, not people: an address on
/// either is a second identity Evolution offers in the composer's From: menu.
#[test]
fn neither_service_is_turned_into_an_identity() {
    let committed = Committed::new(&account());

    for (what, source) in [
        ("the mail account", &committed.account),
        ("the transport", &committed.transport),
    ] {
        assert!(
            !source.has_extension(E_SOURCE_EXTENSION_MAIL_IDENTITY),
            "{what} was given an identity of its own"
        );
    }
}

/// None of the three is ever claimed as a child of the collection *backend*,
/// which is the rule that keeps this writer and `dup_resource_id` from
/// contradicting each other.
///
/// They are children of the account — that is what the parent uid above says —
/// without being cached resources of the backend. The distinction is not
/// cosmetic: `collection_backend_load_resources()` deletes the cache file of
/// any child whose `dup_resource_id` answers NULL, so a mail source this writer
/// had made look like one of ours would be deleted on the next start of the
/// registry.
#[test]
fn no_committed_mail_source_is_read_as_a_child_of_the_collection() {
    let committed = Committed::new(&account());

    for (what, source) in [
        ("the mail account", &committed.account),
        ("the identity", &committed.identity),
        ("the transport", &committed.transport),
    ] {
        // SAFETY: a live source.
        assert_eq!(
            unsafe { resource_id_of(source.0) },
            None,
            "{what} was read as a child the collection backend created"
        );
    }
}

/// Committing an account whose identity changed writes the new address, rather
/// than leaving the old one where it was.
///
/// The same decision as the account source's, and for the same reason: a setup
/// commits onto sources that already say something. An identity left at the old
/// address is not a stale display string — it is the `From:` of every message
/// the user sends afterwards.
#[test]
fn committing_an_account_whose_address_changed_replaces_the_old_one() {
    let committed = Committed::new(&account());

    let second = Account {
        identity: "vera@example.org".to_owned(),
        ..account()
    };
    let committed = committed.written(&second);

    assert_eq!(
        committed.identity.address().as_deref(),
        Some("vera@example.org")
    );
    assert_eq!(
        committed.identity.address(),
        committed.collection.collection_identity(),
    );
    // And the wiring is written once more rather than doubled or dropped.
    assert_eq!(committed.account.identity_uid(), committed.identity.uid());
    assert_eq!(
        committed.identity.transport_uid(),
        committed.transport.uid()
    );
    assert_eq!(committed.account.parent().as_deref(), Some(ACCOUNT_UID));
}

/// An account with mail switched off still gets its three sources, because
/// which parts are on is the account's answer and not these sources'.
///
/// `MailEnabled=false` reaches the mail sources as `enabled`, bound to the
/// account's by the collection backend on every load; sources that were never
/// written at all would instead make "receive mail" a switch the user cannot
/// turn back on without recreating the account.
#[test]
fn the_sources_are_written_whether_or_not_the_account_offers_mail() {
    let mut account = account();
    account.parts = Parts::NONE;
    let committed = Committed::new(&account);

    assert_eq!(
        committed
            .account
            .backend_name(E_SOURCE_EXTENSION_MAIL_ACCOUNT)
            .as_deref(),
        MAIL_BACKEND_NAME.to_str().ok(),
    );
    assert_eq!(committed.account.parent().as_deref(), Some(ACCOUNT_UID));
}

/// What this writer writes is what the registry-side vfunc writes, on all three
/// sources, or one of them is wrong.
///
/// The two exist because they run in different processes with different things
/// in reach — `prepare_mail` has the factory and not the user's answers, this
/// has the answers and no factory — and `e_collection_backend_factory_prepare_mail`
/// has no caller in Evolution 3.52 at all, so in practice this writer is the
/// one that runs. That is exactly why the vfunc has to be held against it:
/// nothing else would notice it going stale, and it is the implementation a
/// later Evolution calling that hook would use.
#[test]
fn the_setup_writes_the_services_the_registry_side_vfunc_would_write() {
    let committed = Committed::new(&account());
    let prepared = Prepared::new();

    for (what, extension, committed, prepared) in [
        (
            "the mail account",
            E_SOURCE_EXTENSION_MAIL_ACCOUNT,
            &committed.account,
            &prepared.account,
        ),
        (
            "the transport",
            E_SOURCE_EXTENSION_MAIL_TRANSPORT,
            &committed.transport,
            &prepared.transport,
        ),
    ] {
        assert_eq!(
            committed.backend_name(extension),
            prepared.backend_name(extension),
            "{what} names a different service depending on which of the two \
             writers got to it"
        );
    }

    // And the same two links, whichever writer made them.
    assert_eq!(
        prepared.account.identity_uid(),
        prepared.identity.uid(),
        "the vfunc's own wiring changed, so this comparison proves nothing"
    );
    assert_eq!(committed.account.identity_uid(), committed.identity.uid());
    assert_eq!(
        committed.identity.transport_uid(),
        committed.transport.uid()
    );
}

/// One account, two writers of `[Security] Method`, and one spelling — or the
/// account changes shape depending on which of them ran last.
///
/// This setup writes the method when the account is committed; the collection
/// backend binds it onto the same two sources from the account, in
/// `evolution-source-registry`, every time `child_added` fires for one of them.
/// Neither can be dropped — the first runs before any registry has seen the
/// account, the second is what keeps a mail source following an account the user
/// edits later — so the constants have to agree. A disagreement would not fail
/// anything visibly: both spellings are strings EDS stores, and only Camel's
/// nick lookup at the far end can tell them apart.
#[test]
fn the_setup_and_the_collection_backend_spell_the_security_method_alike() {
    assert_eq!(
        MAIL_SECURITY_METHOD_TLS,
        mail_child::MAIL_SECURITY_METHOD_TLS
    );
    assert_eq!(
        MAIL_SECURITY_METHOD_NONE,
        mail_child::MAIL_SECURITY_METHOD_NONE
    );
}

/// The mail account names the server the store would connect to, read back
/// through the provider that will do the connecting.
///
/// This is the assertion the whole module exists for, and it is deliberately
/// made at the far end: `ServerConfig::from_settings` is `jmap-mail`'s own
/// reader, called on the very `CamelSettings` object
/// `e_source_camel_configure_service` would hand a `CamelJmapStore`. Everything
/// in between — that `[Authentication] Host` and `[Security] Method` are where
/// an `ESourceCamel` binds host and encryption from, that the two spellings of
/// "encrypted" are not the same string on the two sides — is machinery this test
/// does not have to name, and would fail on if any of it were wrong.
#[test]
fn the_mail_account_names_the_server_the_store_would_connect_to() {
    let committed = Committed::new(&account());

    assert_eq!(
        committed.account.server(),
        Ok(ServerConfig {
            origin: "https://jmap.example.com:8443".to_owned(),
            user: Some("vera".to_owned()),
        })
    );
}

/// And the transport sends through the same server, because there is one
/// server.
///
/// Camel splits an account into a store and a transport with no pointer between
/// them, and configures each from its own `ESource`; JMAP submits over the
/// session it reads through, so the two sources have to say the same thing. A
/// transport left unwritten is not a broken account either — it is an account
/// that receives mail and cannot send it, discovered the first time the user
/// presses Send.
#[test]
fn the_transport_sends_through_the_same_server_the_account_receives_from() {
    let committed = Committed::new(&account());

    assert_eq!(committed.transport.server(), committed.account.server());
    assert!(
        committed.transport.server().is_ok(),
        "both are equally unconfigured, which proves nothing"
    );
}

/// One account, one password: the mail sources are written so that EDS reads
/// their credentials off the collection rather than asking for their own.
///
/// `e_source_credentials_provider_ref_credentials_source` walks a source's
/// parents to the collection and then applies exactly the rule this asserts —
/// `e_util_can_use_collection_as_credential_source`, which compares the two
/// sources' `[Authentication] Host` so that an account may put its outgoing
/// service on a different server with a password of its own. That is a feature
/// for SMTP-beside-IMAP and a trap here: a mail source whose host disagreed with
/// the account's would be a second password prompt for the same server, and a
/// second libsecret entry to get out of sync.
#[test]
fn the_two_mail_services_share_the_password_stored_on_the_account() {
    let committed = Committed::new(&account());

    for (what, source) in [
        ("the mail account", &committed.account),
        ("the transport", &committed.transport),
    ] {
        // The rule has a trivial branch — a child with no `[Authentication]` at
        // all shares by definition — and a mail source cannot take it: host,
        // port and user have nowhere else to live. So this is checked first,
        // or the assertion below would hold for a source that says nothing.
        assert!(
            source.has_extension(E_SOURCE_EXTENSION_AUTHENTICATION),
            "{what} names no server, so sharing a password is not yet the \
             question"
        );
        // SAFETY: two live sources, the first of which carries `[Collection]`.
        let shared = unsafe {
            e_util_can_use_collection_as_credential_source(committed.collection.0, source.0)
        };
        assert_ne!(
            shared, GFALSE,
            "{what} would ask for a password of its own instead of the \
             account's"
        );
    }
}

/// Both readers of a mail source's `[Security] Method` have to agree about what
/// it means, and they read it in different ways.
///
/// The field is a string, and on a mail source it is read twice: EDS derives
/// `ESourceSecurity:secure` from it by comparing against `"none"`, while
/// `ESourceCamel` looks it up as a `CamelNetworkSecurityMethod` **enum nick**.
/// Only the first accepts EDS's own `"tls"`, the spelling `jmap_config::account`
/// writes on the collection — and the way the second one fails is why this test
/// asserts the enum value rather than the connection that comes out of it.
/// `e_binding_transform_enum_nick_to_value` returns `FALSE` on a string that is
/// not a nick and the binding then sets *nothing*, so the settings object keeps
/// the property's default; in EDS 3.52 that default is
/// `STARTTLS_ON_STANDARD_PORT`, which `jmap-mail` — which only distinguishes
/// `NONE` from not — reads as encrypted. An account written as `"tls"` therefore
/// connects perfectly well over TLS while telling Evolution's account editor a
/// setting the user did not choose, and would become a refusal to connect the
/// day Camel's default changed. Asserting the origin alone does not see any of
/// that; asserting the value does.
#[test]
fn both_readers_of_the_security_method_agree_that_the_account_is_encrypted() {
    let committed = Committed::new(&account());

    for (what, source) in [
        ("the mail account", &committed.account),
        ("the transport", &committed.transport),
    ] {
        assert!(source.secure(), "EDS reads {what} as unencrypted");
        assert_eq!(
            source.security_method(),
            CAMEL_NETWORK_SECURITY_METHOD_SSL_ON_ALTERNATE_PORT,
            "Camel reads {what} as some other kind of connection than HTTPS"
        );
        assert_eq!(
            source.server().map(|server| server.origin),
            Ok("https://jmap.example.com:8443".to_owned()),
            "{what} does not assemble the origin a store would open"
        );
    }
}

/// An account the user did not secure reaches its server in plaintext, which is
/// the other half of the same field and only allowed to loopback.
///
/// Written because the interesting spelling is the one that is *not* an enum
/// nick of its own: `"none"` is the string EDS and Camel happen to share, so a
/// writer that got the secure case right by mapping to Camel's vocabulary could
/// still get this one wrong by mapping too eagerly. The host is `localhost`
/// because `jmap_backend_core::source::origin` refuses plaintext anywhere else,
/// and because that is the account the mock server is reached by.
#[test]
fn an_account_the_user_did_not_secure_reaches_its_server_in_plaintext() {
    let account = Account {
        connection: Connection {
            host: "localhost".to_owned(),
            port: Some(8080),
            secure: false,
            ..account().connection
        },
        ..account()
    };
    let committed = Committed::new(&account);

    assert!(!committed.account.secure());
    assert_eq!(
        committed.account.security_method(),
        CAMEL_NETWORK_SECURITY_METHOD_NONE,
        "the account is not encrypted and Camel has to be told so explicitly — \
         its own default for this property is a TLS method"
    );
    assert_eq!(
        committed.account.server(),
        Ok(ServerConfig {
            origin: "http://localhost:8080".to_owned(),
            user: Some("vera".to_owned()),
        })
    );
}

/// Editing an account's server reaches the settings object Camel is already
/// holding, rather than only the keyfile.
///
/// The order matters and is the reverse of every other test here: the
/// `ESourceCamel` extension — and with it the `CamelSettings` instance and the
/// bindings onto `[Authentication]` — is created *before* the commit, which is
/// the real case. A running Evolution has a store with settings on it long
/// before the user opens the account editor, and `G_BINDING_SYNC_CREATE` only
/// explains the first read; what carries an edit afterwards is
/// `G_BINDING_BIDIRECTIONAL`, in that direction, on properties this writer
/// never touches by name.
#[test]
fn moving_an_account_to_another_server_reaches_the_settings_camel_holds() {
    let committed = Committed::new(&account());
    let settings = committed.account.camel_settings();
    assert_eq!(
        committed.account.server().map(|server| server.origin),
        Ok("https://jmap.example.com:8443".to_owned())
    );

    let moved = Account {
        connection: Connection {
            host: "jmap.example.org".to_owned(),
            port: None,
            user: Some("vera.new".to_owned()),
            ..account().connection
        },
        ..account()
    };
    let committed = committed.written(&moved);

    assert_eq!(
        committed.account.camel_settings(),
        settings,
        "the extension was replaced, so this tests a different object than the \
         one a running store would hold"
    );
    assert_eq!(
        committed.account.server(),
        Ok(ServerConfig {
            // No port: the account stopped naming one, so the scheme's default
            // applies rather than the one it named before.
            origin: "https://jmap.example.org".to_owned(),
            user: Some("vera.new".to_owned()),
        })
    );
}

/// An interior NUL in the address is truncated rather than dropping the write.
///
/// Nothing stops a user pasting one and a C string cannot carry it; what must
/// not happen is the field being left unwritten, which for an identity being
/// edited means the *previous* address stays behind — the one case where
/// refusing to write is worse than writing less.
#[test]
fn an_interior_nul_in_the_address_is_truncated_rather_than_lost() {
    let mut account = account();
    account.identity = "vera@example.com\0and more".to_owned();
    let committed = Committed::new(&account);

    assert_eq!(
        committed.identity.address().as_deref(),
        Some("vera@example.com")
    );
}
