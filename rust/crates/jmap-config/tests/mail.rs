// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests the three mail sources that a setup commits with the account, on real
// `ESource`s. They also compare them with the collection backend's
// `prepare_mail`, which writes the same three sources in the registry.
//
// As in `tests/account.rs`, the connection between two parts is the subject,
// but in the opposite direction. There, a different reader had to agree with
// this crate's writer. Here, there are two *writers*. This one runs in
// Evolution's process, which has the user's answers. The vfunc runs in
// `evolution-source-registry`, which has the factory. If tests check two writers
// of one file only separately, the writers drift apart silently. If the mail
// sources name a protocol with no Camel provider, the inbox never opens. No log
// tells which of the two writers wrote the name.
//
// There is also a reader, one source further than the account tests go: the
// `CamelSettings` object that an `ESourceCamel` extension gives a
// `CamelJmapStore`. It is `jmap-mail`'s, not this crate's. Only it answers the
// question that the provider asks an account, because none of the four fields
// that answer it is stored where it seems to be. Hidden machinery binds host,
// port, user and encryption from `[Authentication]` and `[Security]` into a
// settings object. Neither writer mentions it. The binding converts one field
// into a different vocabulary, and silently keeps its own default when the
// conversion fails. Thus these tests generate the subtype, get the settings of
// the extension, and read them with the provider's own `ServerConfig`.

use std::ffi::{CStr, CString, c_char};
use std::ptr;
use std::sync::OnceLock;

use eds_sys::{
    CAMEL_NETWORK_SECURITY_METHOD_NONE, CAMEL_NETWORK_SECURITY_METHOD_SSL_ON_ALTERNATE_PORT,
    CamelNetworkSecurityMethod, CamelSettings, E_SOURCE_EXTENSION_AUTHENTICATION,
    E_SOURCE_EXTENSION_MAIL_ACCOUNT, E_SOURCE_EXTENSION_MAIL_IDENTITY,
    E_SOURCE_EXTENSION_MAIL_SUBMISSION, E_SOURCE_EXTENSION_MAIL_TRANSPORT,
    E_SOURCE_EXTENSION_SECURITY, ESource, ESourceAuthentication, ESourceBackend, ESourceCamel,
    ESourceMailAccount, ESourceMailIdentity, ESourceMailSubmission, ESourceSecurity,
    camel_network_settings_get_security_method, e_collection_backend_factory_prepare_mail,
    e_source_authentication_get_method, e_source_backend_get_backend_name,
    e_source_camel_generate_subtype, e_source_camel_get_extension_name,
    e_source_camel_get_settings, e_source_collection_get_identity, e_source_get_extension,
    e_source_get_parent, e_source_get_uid, e_source_has_extension,
    e_source_mail_account_get_identity_uid, e_source_mail_identity_get_address,
    e_source_mail_submission_get_transport_uid, e_source_new, e_source_new_with_uid,
    e_source_security_get_secure, e_util_can_use_collection_as_credential_source,
};
use glib_sys::GFALSE;
use gobject_sys::{g_object_unref, g_type_create_instance};
use jmap_backend_collection::factory::JmapCollectionFactory;
use jmap_backend_collection::mail_child;
use jmap_backend_collection::resource_id::resource_id_of;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::source::{ConnectTarget, SourceError};
use jmap_backend_core::subclass::register_static;
use jmap_collection_sync::Parts;
use jmap_collection_sync::child_source::Connection;
use jmap_config::account::{Account, apply as apply_account};
use jmap_config::mail::{
    MAIL_BACKEND_NAME, MAIL_SECURITY_METHOD_NONE, MAIL_SECURITY_METHOD_TLS, MailSources, apply,
};
use jmap_mail::server::ServerConfig;
use jmap_mail::settings::settings_type;

/// The uid of the account source. Thus the test compares the parent of the
/// three mail sources with a known string, not one read from the checked source.
const ACCOUNT_UID: &str = "jmap-account";

/// Generates the `ESourceCamel` subtype for `jmap`, then returns its extension
/// name `[JMAP Backend]`, which contains the Camel settings of a `jmap` service.
///
/// Usually `e_source_camel_register_types()` does this. It loads each installed
/// Camel provider module and generates one subtype for each service class in it.
/// Here the provider is linked, not installed, so this generates the one subtype
/// directly. `e_source_camel_generate_subtype` documents this case as the reason
/// that it is public API. M7's module must also do this, and Evolution's own
/// account editor need not, for the same reason: nothing has loaded
/// `libcameljmap.so` when the setup writes an account.
///
/// The `OnceLock` is necessary. `generate_subtype` reads the type name and then
/// registers it in two steps, and Rust runs the tests of this file as threads of
/// one process. If a thread loses that race, GObject aborts. The name is a
/// `usize` because a raw pointer is not `Sync`. The string itself is
/// `g_intern_string`'d, so it lives as long as the process.
fn camel_extension_name() -> *const c_char {
    static NAME: OnceLock<usize> = OnceLock::new();
    let name = *NAME.get_or_init(|| {
        // SAFETY: a NUL-terminated protocol name and a GType derived from
        // CamelSettings, which `settings_type` registers. The returned name is
        // interned and never freed.
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
    /// A blank source with a new uid, as the setup's scratch mail sources are.
    /// EDS itself uses `e_source_new (NULL, NULL, &error)` for a source with no
    /// keyfile yet.
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

    /// `[Authentication] Method`: jmap-mail and the backends read it (through
    /// `CamelNetworkSettings:auth-mechanism`) to select Basic, Bearer or OAuth 2.0.
    fn authentication_method(&self) -> Option<String> {
        // SAFETY: a live source. The extension is created on demand and the
        // source owns it. The getter's string lives longer than this read.
        unsafe {
            let auth: *mut ESourceAuthentication =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast();
            read_string(e_source_authentication_get_method(auth))
        }
    }

    fn has_extension(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a NUL-terminated header constant.
        unsafe { e_source_has_extension(self.0, name.as_ptr()) != GFALSE }
    }

    /// The `ESourceBackend:backend-name` under `name`, read only if the extension
    /// is present. `e_source_get_extension` creates an extension that it cannot
    /// find. Without the check, "the writer wrote nothing" would become "the
    /// writer wrote NULL".
    fn backend_name(&self, name: &CStr) -> Option<String> {
        assert!(
            self.has_extension(name),
            "the source has no {name:?} to read a backend name from"
        );
        // SAFETY: the extension is present, so this returns the source's own
        // extension, which the source owns. Its name is NULL or a NUL-terminated
        // string with the same lifetime.
        unsafe {
            let backend: *mut ESourceBackend = e_source_get_extension(self.0, name.as_ptr()).cast();
            read_string(e_source_backend_get_backend_name(backend))
        }
    }

    /// `[Mail Account] IdentityUid`: the identity that this receiving account
    /// sends as.
    fn identity_uid(&self) -> Option<String> {
        assert!(
            self.has_extension(E_SOURCE_EXTENSION_MAIL_ACCOUNT),
            "the source is not a mail account"
        );
        // SAFETY: the extension is present. Its uid is NULL or a NUL-terminated
        // string that it owns.
        unsafe {
            let account: *mut ESourceMailAccount =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_MAIL_ACCOUNT.as_ptr()).cast();
            read_string(e_source_mail_account_get_identity_uid(account))
        }
    }

    /// `[Mail Submission] TransportUid`: the service that sends this identity's
    /// mail.
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

    /// `[Mail Identity] Address`: the sender address of sent mail.
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

    /// The `CamelSettings` object that a Camel service configured from this
    /// source gets.
    ///
    /// These are the two steps of `e_source_camel_configure_service`, without
    /// the service: get the provider's extension from the source, then get the
    /// settings from the extension. The result is the same object that a
    /// `CamelJmapStore` gets, with host, port, user and security. None of those
    /// four is stored *in* the extension. `ESourceCamel` binds them in two
    /// directions to `[Authentication]` and `[Security]` on this source. Thus the
    /// setup writes those groups, and only this reader proves that it did.
    fn camel_settings(&self) -> *mut CamelSettings {
        // SAFETY: a live source and the interned extension name of a
        // registered `ESourceCamel` subtype. The extension is created on demand,
        // and the source owns it and its settings object.
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

    /// The `CamelNetworkSecurityMethod` of those settings: the converted value of
    /// `[Security] Method`, not the string. A wrong spelling fails silently in
    /// the conversion.
    fn security_method(&self) -> CamelNetworkSecurityMethod {
        // SAFETY: the settings object of a live source. It implements
        // `CamelNetworkSettings`: `CamelJmapSettings` claims the interface. An
        // `ESourceCamel` subtype from a settings class without it would have
        // nothing to bind.
        unsafe { camel_network_settings_get_security_method(self.camel_settings().cast()) }
    }

    /// The server that `jmap-mail` reads from those settings. Its `connect_sync`
    /// needs this last, before it builds a client. This is the data that the
    /// source must supply.
    fn server(&self) -> Result<ServerConfig, SourceError> {
        // SAFETY: the settings object of a live source, only read from.
        unsafe { ServerConfig::from_settings(self.camel_settings()) }
    }

    /// `[Security] Secure`: the boolean that EDS derives from the method string.
    /// All non-mail readers of this account read it.
    fn secure(&self) -> bool {
        assert!(
            self.has_extension(E_SOURCE_EXTENSION_SECURITY),
            "the source says nothing about transport security"
        );
        // SAFETY: the extension is present, so this returns the source's own
        // extension.
        unsafe {
            let security: *mut ESourceSecurity =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_SECURITY.as_ptr()).cast();
            e_source_security_get_secure(security) != GFALSE
        }
    }

    /// `[Collection] Identity` of the account source: the other location of the
    /// account's address. The two addresses must agree.
    fn collection_identity(&self) -> Option<String> {
        // SAFETY: `apply_account` created the extension, so this returns the
        // source's own extension.
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

/// An account and its four sources, after the setup wrote all of them.
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

    /// The steps of a commit, in sequence: the account source first, then the
    /// three below it.
    fn written(self, account: &Account) -> Self {
        // SAFETY: four live sources. This call keeps none of them.
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

/// The same three sources as the registry writes them: blank sources given to
/// the collection factory's `prepare_mail` vfunc, called as EDS calls it.
struct Prepared {
    account: Source,
    identity: Source,
    transport: Source,
}

impl Prepared {
    fn new() -> Self {
        // Registered statically, not through the module entry point, as in the
        // tests of `jmap-backend-collection`. There is no module here for the
        // type, and `register_static` returns an already-registered type.
        let gtype = register_static::<JmapCollectionFactory>();
        assert_ne!(gtype, 0, "the factory type is not registered");
        // `g_type_create_instance`, not `g_object_new`: `EExtension:extensible`
        // is CONSTRUCT_ONLY, and `g_object_new` with no properties logs an
        // unrelated critical.
        // SAFETY: a registered, instantiatable type.
        let factory = unsafe { g_type_create_instance(gtype) };
        assert!(!factory.is_null(), "g_type_create_instance returned NULL");

        let prepared = Self {
            account: Source::blank(),
            identity: Source::blank(),
            transport: Source::blank(),
        };
        // SAFETY: a live factory of a type derived from
        // ECollectionBackendFactory, and three live sources. The call keeps
        // none of them.
        unsafe {
            e_collection_backend_factory_prepare_mail(
                factory.cast(),
                prepared.account.0,
                prepared.identity.0,
                prepared.transport.0,
            );
        }
        // SAFETY: the reference from instance creation, released one time.
        unsafe { g_object_unref(factory.cast()) };

        prepared
    }
}

/// An account with all fields set, so that a string written on the wrong
/// source is visible.
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

/// The parent makes the three sources *this account's* mail, not three
/// unattached sources in the registry's directory.
///
/// `e_collection_backend_list_mail_sources()` finds them through the account's
/// children, and `collection_backend_bind_child_enabled()` binds the `enabled`
/// of each to the account's `mail-enabled` in the same pass. A mail source
/// with no parent is not a broken account. It is a second, top-level account in
/// the sidebar that the collection does not know. No "receive mail for this
/// account" switch controls it.
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

/// The mail services inherit the account's authentication method. Thus a
/// Bearer (or OAuth 2.0) account does not silently use Basic on its mail child.
/// Regression: `apply_server` wrote a blank `[Authentication] Method`. An
/// API-token Fastmail account then connected its collection over Bearer, but
/// its mail child asked for a password again and again.
#[test]
fn the_mail_services_inherit_the_authentication_method() {
    let mut account = account();
    account.connection.auth_method = Some("bearer".to_owned());
    let committed = Committed::new(&account);

    for (what, source) in [
        ("the mail account", &committed.account),
        ("the transport", &committed.transport),
    ] {
        assert_eq!(
            source.authentication_method().as_deref(),
            Some("bearer"),
            "{what} did not inherit the account's Bearer auth method"
        );
    }
}

/// Evolution receives through the mail account. Its backend name is the Camel
/// protocol of `libcameljmap.so`.
///
/// A mail account source without it causes no error. Evolution shows the
/// account but cannot open it, because it calls `camel_provider_get` for the
/// empty protocol.
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

/// The transport uses the same provider, with the *same* name.
///
/// JMAP submits over the session that it reads through. Thus `jmap-mail`'s
/// provider registers one protocol with a store type and a transport type. There
/// is no second service, as `smtp` is beside `imapx`.
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

/// The two links that make three sources one account. The account names the
/// identity that it sends as. The identity names the transport that sends its
/// mail.
///
/// Both are uids, and the test compares them with the uids of the named sources.
/// A link to the wrong uid does not fail at commit time. It causes a `From:`
/// header from a different account, or a send through a different account's
/// server.
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

/// The sender address of the identity is the address of the account. The user
/// typed it one time.
///
/// EDS puts `[Collection] Identity` and `[Mail Identity] Address` in two
/// different sources, and this crate cannot change that. But this crate writes
/// both from one string. If the two disagree, the setup sends mail from an
/// address that the account does not claim.
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

/// The identity is a person, not a service. A service extension would make it
/// a service.
///
/// `collection_backend_child_is_mail()` treats a source with one of the three
/// mail extensions as mail. Thus an identity that also had `[Mail Account]`
/// would be a second, empty receiving account of this user. Evolution would
/// show it, but it would connect to nothing.
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

/// The account and the transport are services, not people. An address on one
/// of them adds a second identity to the composer's From: menu.
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

/// None of the three is a child of the collection *backend*. This rule keeps
/// this writer and `dup_resource_id` in agreement.
///
/// They are children of the account (the parent uid above), but not cached
/// resources of the backend. The difference is important:
/// `collection_backend_load_resources()` deletes the cache file of each child
/// whose `dup_resource_id` returns NULL. If this writer made a mail source look
/// like a backend resource, the next registry start would delete it.
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

/// A commit of an account with a changed identity writes the new address, and
/// does not keep the old one.
///
/// The account source does the same, for the same reason: a setup commits to
/// sources that already contain values. An old address on the identity is not
/// only a stale display string. It is the `From:` of each message that the user
/// sends after it.
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
    // The links are written again, not duplicated or removed.
    assert_eq!(committed.account.identity_uid(), committed.identity.uid());
    assert_eq!(
        committed.identity.transport_uid(),
        committed.transport.uid()
    );
    assert_eq!(committed.account.parent().as_deref(), Some(ACCOUNT_UID));
}

/// An account with mail off still gets its three sources. The account, not
/// these sources, records which parts are on.
///
/// `MailEnabled=false` gets to the mail sources as `enabled`, which the
/// collection backend binds to the account's value on each load. Without the
/// sources, the user cannot enable "receive mail" again without a new account.
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

/// This writer writes the same data as the registry-side vfunc, on all three
/// sources. If not, one of them is wrong.
///
/// There are two writers because they run in different processes with access to
/// different data. `prepare_mail` has the factory but not the user's answers.
/// This writer has the answers but no factory.
/// `e_collection_backend_factory_prepare_mail` has no caller in Evolution 3.52,
/// so only this writer runs. Thus the test compares the vfunc with it. Nothing
/// else detects when the vfunc becomes stale, and a later Evolution that calls
/// that hook uses it.
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

    // The same two links, from each writer.
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

/// One account has two writers of `[Security] Method`, which must use one
/// spelling. If not, the account changes when the other writer runs.
///
/// This setup writes the method when it commits the account. In
/// `evolution-source-registry`, the collection backend binds it from the account
/// to the same two sources each time `child_added` runs for one of them. Both
/// are necessary. The first runs before a registry knows the account. The second
/// updates a mail source when the user edits the account later. Thus the
/// constants must agree. A difference causes no visible failure: EDS stores both
/// spellings as strings, and only Camel's nick lookup at the end sees the
/// difference.
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

/// The mail account names the server for the store. The provider that
/// connects reads it.
///
/// This is the main assertion of the module, and it is deliberately made at the
/// end of the chain. `ServerConfig::from_settings` is `jmap-mail`'s own reader,
/// called on the same `CamelSettings` object that
/// `e_source_camel_configure_service` gives a `CamelJmapStore`. This test does
/// not name the machinery between them. `ESourceCamel` binds host and
/// encryption from `[Authentication] Host` and `[Security] Method`. The two
/// sides spell "encrypted" differently. If one of these is wrong, the test fails.
#[test]
fn the_mail_account_names_the_server_the_store_would_connect_to() {
    let committed = Committed::new(&account());

    assert_eq!(
        committed.account.server(),
        Ok(ServerConfig {
            target: ConnectTarget::Origin("https://jmap.example.com:8443".to_owned()),
            user: Some("vera".to_owned()),
        })
    );
}

/// The transport sends through the same server, because there is one server.
///
/// Camel divides an account into a store and a transport with no pointer
/// between them, and configures each from its own `ESource`. JMAP submits over
/// the session it reads through, so the two sources must agree. An unwritten
/// transport is also not a broken account. The account receives mail but cannot
/// send it, and the user sees this the first time they push Send.
#[test]
fn the_transport_sends_through_the_same_server_the_account_receives_from() {
    let committed = Committed::new(&account());

    assert_eq!(committed.transport.server(), committed.account.server());
    assert!(
        committed.transport.server().is_ok(),
        "both are equally unconfigured, which proves nothing"
    );
}

/// One account has one password. EDS reads the credentials of the mail sources
/// from the collection and does not ask for separate ones.
///
/// `e_source_credentials_provider_ref_credentials_source` follows a source's
/// parents to the collection. It then applies the rule that this test asserts:
/// `e_util_can_use_collection_as_credential_source`. That function compares the
/// `[Authentication] Host` of the two sources, so that an account can put its
/// outgoing service on a different server with its own password. This helps
/// SMTP beside IMAP, but here it is a risk. A mail source with a different host
/// causes a second password prompt for the same server. It also causes a second
/// libsecret entry that can become out of sync.
#[test]
fn the_two_mail_services_share_the_password_stored_on_the_account() {
    let committed = Committed::new(&account());

    for (what, source) in [
        ("the mail account", &committed.account),
        ("the transport", &committed.transport),
    ] {
        // The rule has a trivial branch: a child with no `[Authentication]`
        // always shares. A mail source must not use it, because host, port and
        // user have no other location. Thus check this first. If not, the
        // assertion below is also true for an empty source.
        assert!(
            source.has_extension(E_SOURCE_EXTENSION_AUTHENTICATION),
            "{what} names no server, so sharing a password is not yet the \
             question"
        );
        // SAFETY: two live sources. The first has `[Collection]`.
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

/// The two readers of a mail source's `[Security] Method` must agree on its
/// meaning, and they read it differently.
///
/// The field is a string, and two readers read it on a mail source. EDS derives
/// `ESourceSecurity:secure` from it by comparison with `"none"`. `ESourceCamel`
/// reads it as a `CamelNetworkSecurityMethod` **enum nick**. Only the first
/// accepts EDS's own `"tls"`, which `jmap_config::account` writes on the
/// collection. Because of how the second fails, this test asserts the enum
/// value, not only the resulting connection.
/// `e_binding_transform_enum_nick_to_value` returns `FALSE` for a string that is
/// not a nick, and the binding then sets *nothing*. Thus the settings object
/// keeps the property's default. In EDS 3.52 that default is
/// `STARTTLS_ON_STANDARD_PORT`. `jmap-mail` only compares with `NONE`, so it
/// reads that as encrypted. An account written as `"tls"` thus connects
/// correctly over TLS. But Evolution's account editor shows a setting that the
/// user did not select, and the account refuses to connect if Camel's default
/// changes. An assertion on the origin only does not see this. An assertion on
/// the value does.
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
            source.server().map(|server| server.target),
            Ok(ConnectTarget::Origin(
                "https://jmap.example.com:8443".to_owned()
            )),
            "{what} does not assemble the origin a store would open"
        );
    }
}

/// An account that the user did not secure connects to its server in
/// plaintext. This is the other half of the same field, and only loopback
/// permits it.
///
/// This case is important because `"none"` is the spelling that EDS and Camel
/// share. A writer that maps the secure case correctly to Camel's vocabulary
/// can still map this case incorrectly if it maps all values. The host is
/// `localhost` because `jmap_backend_core::source::origin` refuses plaintext to
/// all other hosts, and because the mock server uses that account.
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
            target: ConnectTarget::Origin("http://localhost:8080".to_owned()),
            user: Some("vera".to_owned()),
        })
    );
}

/// A change to an account's server updates the settings object that Camel
/// already has, not only the keyfile.
///
/// The sequence is important, and it is the opposite of all other tests here.
/// The `ESourceCamel` extension (with the `CamelSettings` instance and the
/// bindings to `[Authentication]`) exists *before* the commit, as in real use.
/// A running Evolution has a store with settings long before the user opens the
/// account editor. `G_BINDING_SYNC_CREATE` only explains the first read. After
/// that, `G_BINDING_BIDIRECTIONAL` transfers an edit, in that direction, on
/// properties that this writer never names.
#[test]
fn moving_an_account_to_another_server_reaches_the_settings_camel_holds() {
    let committed = Committed::new(&account());
    let settings = committed.account.camel_settings();
    assert_eq!(
        committed.account.server().map(|server| server.target),
        Ok(ConnectTarget::Origin(
            "https://jmap.example.com:8443".to_owned()
        ))
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
            // No port: the account no longer names one. This is the RFC 8620
            // §2.2 form, which SRV autodiscovery can use, not only a scheme
            // default.
            target: ConnectTarget::Domain("jmap.example.org".to_owned()),
            user: Some("vera.new".to_owned()),
        })
    );
}

/// An interior NUL truncates the address, and the address is still written.
///
/// A user can paste one, and a C string cannot contain it. The field must not
/// stay unwritten: on an edited identity, the *previous* address would stay.
/// Here, a refusal to write is worse than a shorter write.
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
