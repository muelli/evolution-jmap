// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The mail sources of an account, following the account — the case
// `tests/child_added.rs` deliberately does not cover, because a mail source is
// not a child this backend created and does not look like one.
//
// Two things are asserted here that nothing else can assert. The first is that
// the transport gets a server at all: Evolution's assistant mints it, the setup
// module writes the service name on it and can write no host — the sending page
// is hidden for a store-and-transport provider, so nothing in the dialog is ever
// asked where it sends through — and this backend is the only place holding both
// the account and its mail children. The second is the *spelling* of
// `[Security] Method` on a mail source, which is a Camel enum nick rather than
// EDS's own two words; a plain `secure` binding writes `"tls"` there, which is
// not one of Camel's nicks and so silently leaves the store's security method at
// whatever `CamelNetworkSettings` defaults to.
//
// The account and the mail sources are real `ESource`s, and the account is
// changed through EDS's own setters afterwards, so what is asserted is a live
// `GBinding` doing its work rather than a copy this test took.

use std::ffi::{CStr, CString, c_char};
use std::ptr;
use std::sync::OnceLock;

use eds_sys::{
    CAMEL_NETWORK_SECURITY_METHOD_NONE, CAMEL_NETWORK_SECURITY_METHOD_SSL_ON_ALTERNATE_PORT,
    CamelNetworkSecurityMethod, CamelSettings, E_SOURCE_EXTENSION_AUTHENTICATION,
    E_SOURCE_EXTENSION_MAIL_ACCOUNT, E_SOURCE_EXTENSION_MAIL_IDENTITY,
    E_SOURCE_EXTENSION_MAIL_TRANSPORT, E_SOURCE_EXTENSION_SECURITY, ESource, ESourceAuthentication,
    ESourceBackend, ESourceCamel, ESourceSecurity, camel_network_settings_get_security_method,
    e_source_authentication_get_host, e_source_authentication_get_method,
    e_source_authentication_get_port, e_source_authentication_get_type,
    e_source_authentication_get_user, e_source_authentication_set_host,
    e_source_authentication_set_method, e_source_authentication_set_port,
    e_source_authentication_set_user, e_source_backend_set_backend_name,
    e_source_camel_generate_subtype, e_source_camel_get_extension_name,
    e_source_camel_get_settings, e_source_get_extension, e_source_has_extension,
    e_source_mail_account_get_type, e_source_mail_identity_get_type,
    e_source_mail_transport_get_type, e_source_new_with_uid, e_source_security_get_method,
    e_source_security_get_type, e_source_security_set_method, e_source_security_set_secure,
};
use glib_sys::{GFALSE, GTRUE};
use gobject_sys::g_object_unref;
use jmap_backend_collection::child_added::follow_collection;
use jmap_backend_collection::mail_child::{
    MAIL_SECURITY_METHOD_NONE, MAIL_SECURITY_METHOD_TLS, mail_service_of,
};
use jmap_backend_collection::prepare_mail::MAIL_BACKEND_NAME;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::source::{ConnectTarget, SourceError};
use jmap_mail::server::ServerConfig;
use jmap_mail::settings::settings_type;

mod common;
use common::{with_timeout, with_timeout_duration};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The name `[JMAP Backend]` — the extension a `jmap` service's Camel settings
/// live under — having first generated the `ESourceCamel` subtype that carries
/// it.
///
/// The same helper, for the same reason and with the same `OnceLock`, as
/// `jmap-config`'s `tests/mail.rs`: the provider is linked into this test rather
/// than installed, so nothing has called `e_source_camel_register_types()` and
/// the one subtype these tests need is generated directly. Losing the race
/// between generating and registering it is an abort inside GObject, and Rust
/// runs the tests in this file as threads of one process.
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

/// An `ESource` this test holds one reference to, with the extensions each test
/// puts on it.
struct Source(*mut ESource);

impl Source {
    fn new(uid: &str) -> Self {
        // SAFETY: no arguments; `e_source_get_extension` and
        // `e_source_has_extension` cannot find an extension class whose type
        // nothing has referenced yet.
        unsafe {
            e_source_authentication_get_type();
            e_source_security_get_type();
            e_source_mail_account_get_type();
            e_source_mail_identity_get_type();
            e_source_mail_transport_get_type();
        }
        let uid = CString::new(uid).expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a GError
        // out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    /// The account, as `jmap_config::account::apply` leaves it: a server, a
    /// user, the credentials-provider method EDS resolves the password through,
    /// and TLS.
    fn account() -> Self {
        let source = Self::new("jmap-account");
        source.set_host("jmap.example.com");
        source.set_port(8443);
        source.set_user("vera@example.com");
        source.set_auth_method("plain/password");
        source.set_secure(true);
        source
    }

    /// One of the two mail service sources, as the assistant mints it and the
    /// setup module names it: a service and nothing else — no `[Authentication]`
    /// and no `[Security]`, which is the state the transport really reaches this
    /// backend in.
    fn service(extension: &CStr, backend_name: &CStr) -> Self {
        let source = Self::new("jmap-mail-service");
        // SAFETY: a live source and a header constant; the extension is created
        // on demand and owned by the source, and the setter copies the string.
        unsafe {
            let backend: *mut ESourceBackend =
                e_source_get_extension(source.0, extension.as_ptr()).cast();
            e_source_backend_set_backend_name(backend, backend_name.as_ptr());
        }
        source
    }

    /// The third of the three, which names no service because it is a person.
    fn identity() -> Self {
        let source = Self::new("jmap-mail-identity");
        // SAFETY: a live source and a header constant.
        unsafe {
            assert!(
                !e_source_get_extension(source.0, E_SOURCE_EXTENSION_MAIL_IDENTITY.as_ptr())
                    .is_null()
            );
        }
        source
    }

    fn has(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a header constant.
        unsafe { e_source_has_extension(self.0, name.as_ptr()) != GFALSE }
    }

    /// `[Authentication]`, created on demand — this is a test writing an
    /// account, not the backend reading one.
    fn authentication(&self) -> *mut ESourceAuthentication {
        // SAFETY: a live source and a header constant.
        unsafe { e_source_get_extension(self.0, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast() }
    }

    fn security(&self) -> *mut ESourceSecurity {
        // SAFETY: as above.
        unsafe { e_source_get_extension(self.0, E_SOURCE_EXTENSION_SECURITY.as_ptr()).cast() }
    }

    fn set_host(&self, host: &str) {
        let host = CString::new(host).expect("no NUL in a literal");
        // SAFETY: a live extension; the setter copies the string.
        unsafe { e_source_authentication_set_host(self.authentication(), host.as_ptr()) };
    }

    fn set_port(&self, port: u16) {
        // SAFETY: a live extension.
        unsafe { e_source_authentication_set_port(self.authentication(), port) };
    }

    fn set_user(&self, user: &str) {
        let user = CString::new(user).expect("no NUL in a literal");
        // SAFETY: as above.
        unsafe { e_source_authentication_set_user(self.authentication(), user.as_ptr()) };
    }

    fn set_auth_method(&self, method: &str) {
        let method = CString::new(method).expect("no NUL in a literal");
        // SAFETY: as above.
        unsafe { e_source_authentication_set_method(self.authentication(), method.as_ptr()) };
    }

    fn set_secure(&self, secure: bool) {
        // SAFETY: as above.
        unsafe {
            e_source_security_set_secure(self.security(), if secure { GTRUE } else { GFALSE });
        }
    }

    fn set_security_method(&self, method: &CStr) {
        // SAFETY: as above.
        unsafe { e_source_security_set_method(self.security(), method.as_ptr()) };
    }

    fn host(&self) -> Option<String> {
        // SAFETY: a live extension; the string is owned by it.
        unsafe { read_string(e_source_authentication_get_host(self.authentication())) }
    }

    fn port(&self) -> u16 {
        // SAFETY: as above.
        unsafe { e_source_authentication_get_port(self.authentication()) }
    }

    fn user(&self) -> Option<String> {
        // SAFETY: as above.
        unsafe { read_string(e_source_authentication_get_user(self.authentication())) }
    }

    fn auth_method(&self) -> Option<String> {
        // SAFETY: as above.
        unsafe { read_string(e_source_authentication_get_method(self.authentication())) }
    }

    fn security_method(&self) -> Option<String> {
        // SAFETY: as above.
        unsafe { read_string(e_source_security_get_method(self.security())) }
    }

    /// The `CamelSettings` object a Camel service configured from this source
    /// would be given — `e_source_camel_configure_service`'s own two steps with
    /// the service left out.
    ///
    /// None of host, port, user or security method is stored *in* that
    /// extension: `ESourceCamel` binds all of them to `[Authentication]` and
    /// `[Security]` on this same source, which is why this module writes those
    /// groups and why this is the only reader that proves it did.
    fn camel_settings(&self) -> *mut CamelSettings {
        // SAFETY: a live source and the interned extension name of a registered
        // `ESourceCamel` subtype; the extension is created on demand and owned
        // by the source, and so is the settings object it holds.
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
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: this holds the only reference.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// The two sources that name a service, which are the two this module writes
/// on: the account Evolution receives through and the transport it sends
/// through.
const SERVICES: [&CStr; 2] = [
    E_SOURCE_EXTENSION_MAIL_ACCOUNT,
    E_SOURCE_EXTENSION_MAIL_TRANSPORT,
];

#[test]
fn a_mail_source_of_this_account_is_recognised_by_its_service_and_its_provider() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for extension in SERVICES {
            let source = Source::service(extension, MAIL_BACKEND_NAME);
            // SAFETY: a live source.
            assert_eq!(
                unsafe { mail_service_of(source.0) },
                Some(extension),
                "{extension:?} naming this provider is one of this account's mail services"
            );
        }

        // The identity is a person rather than a service, and the third source of
        // every mail account: reading it as one would put an `[Authentication]`
        // group on it and, through `collection_backend_child_is_mail`, a second
        // receiving account in Evolution's list.
        let identity = Source::identity();
        // SAFETY: a live source.
        assert_eq!(unsafe { mail_service_of(identity.0) }, None);

        // And a service of somebody else's, which is the case
        // `e_util_can_use_collection_as_credential_source` exists for: an account
        // may send through a server of its own, with a password of its own, and a
        // backend that wrote this collection's host onto it would break both.
        let smtp = Source::service(E_SOURCE_EXTENSION_MAIL_TRANSPORT, c"smtp");
        // SAFETY: a live source.
        assert_eq!(unsafe { mail_service_of(smtp.0) }, None);
    });
}

#[test]
fn the_mail_sources_the_assistant_leaves_behind_reach_the_accounts_server() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The gap this module exists for. Evolution's assistant creates the three
        // scratch sources and writes the service name; the setup module's
        // `commit_changes` can write the server on the *receiving* source only,
        // because the sending page is hidden for a store-and-transport provider and
        // its backend is never asked. So a transport arrives here naming `jmap` and
        // no host at all.
        let account = Source::account();

        for extension in SERVICES {
            let service = Source::service(extension, MAIL_BACKEND_NAME);
            assert!(
                !service.has(E_SOURCE_EXTENSION_AUTHENTICATION),
                "the test did not start from a source that names no server"
            );

            // SAFETY: two live sources.
            unsafe { follow_collection(account.0, service.0) };

            assert_eq!(service.host().as_deref(), Some("jmap.example.com"));
            assert_eq!(service.port(), 8443);
            assert_eq!(service.user().as_deref(), Some("vera@example.com"));
        }
    });
}

#[test]
fn a_mail_source_carries_camels_own_spelling_of_the_security_method() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // EDS's `secure` boolean writes one of two words that Camel does not know:
        // pinned here, because it is the whole reason this module cannot bind
        // `secure` onto a mail source the way `child_added` binds it onto an address
        // book.
        let plain = Source::new("jmap-eds-spelling");
        plain.set_secure(true);
        assert_eq!(
            plain.security_method().as_deref(),
            Some("tls"),
            "EDS no longer writes the spelling this module works around"
        );

        let account = Source::account();
        let transport = Source::service(E_SOURCE_EXTENSION_MAIL_TRANSPORT, MAIL_BACKEND_NAME);

        // SAFETY: two live sources.
        unsafe { follow_collection(account.0, transport.0) };

        // The literal rather than the constant, here and in the test below: a
        // constant compared against itself would go on passing through any rename of
        // the string it stands for, which is the one thing that can go wrong with it.
        assert_eq!(
            transport.security_method().as_deref(),
            Some("ssl-on-alternate-port"),
            "the mail source names a security method `CamelNetworkSettings` cannot \
             read, and so connects with whatever Camel defaults to"
        );
        assert_eq!(
            MAIL_SECURITY_METHOD_TLS.to_str().ok(),
            Some("ssl-on-alternate-port"),
        );
    });
}

#[test]
fn the_camel_spelling_a_commit_wrote_is_not_replaced_by_the_accounts() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The other direction of the same difference, and a regression: the mail
        // account source arrives from `jmap_config::mail::apply_server` already
        // carrying Camel's nick, and a `secure` binding would overwrite it with
        // EDS's `"tls"` the moment `G_BINDING_SYNC_CREATE` fires.
        let account = Source::account();
        let mail_account = Source::service(E_SOURCE_EXTENSION_MAIL_ACCOUNT, MAIL_BACKEND_NAME);
        mail_account.set_security_method(MAIL_SECURITY_METHOD_TLS);

        // SAFETY: two live sources.
        unsafe { follow_collection(account.0, mail_account.0) };

        assert_eq!(
            mail_account.security_method().as_deref(),
            Some("ssl-on-alternate-port"),
            "what the setup committed was replaced by a spelling Camel cannot read"
        );
    });
}

#[test]
fn switching_tls_off_on_the_account_reaches_its_mail_sources() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The mock server, and every plaintext account: a mail source left claiming
        // encryption against a server that offers none does not connect at all.
        let account = Source::account();
        let transport = Source::service(E_SOURCE_EXTENSION_MAIL_TRANSPORT, MAIL_BACKEND_NAME);

        // SAFETY: two live sources.
        unsafe { follow_collection(account.0, transport.0) };
        account.set_secure(false);

        assert_eq!(
            transport.security_method().as_deref(),
            Some("none"),
            "the mail source still claims a security the account no longer has"
        );
        assert_eq!(MAIL_SECURITY_METHOD_NONE.to_str().ok(), Some("none"));
    });
}

#[test]
fn moving_the_account_to_another_server_moves_its_mail_sources() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // A binding rather than a copy, for the reason `child_added` gives: nothing
        // re-runs a commit, so an account the user moves is one whose mail sources
        // go on reaching the old server — and EDS decides whether the mail source
        // shares the account's password by comparing exactly these two host
        // strings, so a stale one is also a second password prompt.
        let account = Source::account();
        let transport = Source::service(E_SOURCE_EXTENSION_MAIL_TRANSPORT, MAIL_BACKEND_NAME);

        // SAFETY: two live sources.
        unsafe { follow_collection(account.0, transport.0) };

        account.set_host("jmap.example.org");
        account.set_port(443);
        account.set_user("vera@example.org");

        assert_eq!(transport.host().as_deref(), Some("jmap.example.org"));
        assert_eq!(transport.port(), 443);
        assert_eq!(transport.user().as_deref(), Some("vera@example.org"));
    });
}

#[test]
fn the_accounts_authentication_method_reaches_its_mail_sources() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `[Authentication] Method` is followed like host/port/user. `jmap-mail`
        // reuses it (via `CamelNetworkSettings:auth-mechanism`) as this project's
        // credential-type selector — `uses_api_token`/`uses_oauth2` read it to
        // choose Basic vs Bearer vs OAuth 2.0 — so a mail source that did not follow
        // it would always authenticate as Basic. That is exactly why a Bearer
        // (API-token) account's transport re-prompted for a password forever while
        // its receiving account, whose method is written directly, connected.
        let account = Source::account();
        let transport = Source::service(E_SOURCE_EXTENSION_MAIL_TRANSPORT, MAIL_BACKEND_NAME);

        // SAFETY: two live sources.
        unsafe { follow_collection(account.0, transport.0) };

        // The account's Bearer choice must reach the transport, live (the binding
        // syncs on create and on every later change).
        account.set_auth_method("bearer");
        assert_eq!(
            transport.auth_method().as_deref(),
            Some("bearer"),
            "the transport did not follow the account's Bearer authentication method"
        );

        // And a change back to the password method still propagates.
        account.set_auth_method("none");
        assert_eq!(
            transport.auth_method().as_deref(),
            Some("none"),
            "the transport did not follow the account back to the password method"
        );
    });
}

#[test]
fn the_transport_sends_through_the_server_the_provider_would_connect_to() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The assertion this module exists for, made at the far end: not that four
        // keys were written, but that the object `e_source_camel_configure_service`
        // hands a `CamelJmapTransport` names this account's server — read by
        // `jmap-mail`'s own reader, which is what `connect_sync` calls. Everything
        // in between is machinery this test does not have to name and would fail on
        // if any of it were wrong.
        let account = Source::account();
        let transport = Source::service(E_SOURCE_EXTENSION_MAIL_TRANSPORT, MAIL_BACKEND_NAME);

        // SAFETY: two live sources.
        unsafe { follow_collection(account.0, transport.0) };

        assert_eq!(
            transport.server(),
            Ok(ServerConfig {
                target: ConnectTarget::Origin("https://jmap.example.com:8443".to_owned()),
                user: Some("vera@example.com".to_owned()),
            })
        );
        assert_eq!(
            transport.camel_security_method(),
            CAMEL_NETWORK_SECURITY_METHOD_SSL_ON_ALTERNATE_PORT,
        );
    });
}

#[test]
fn a_plaintext_account_reaches_its_transport_as_plaintext() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The case a wrong `[Security] Method` spelling is *visible* in, and so the
        // one that keeps this module honest. An unreadable nick leaves the settings
        // object at `CamelNetworkSettings`' own default — `STARTTLS_ON_STANDARD_PORT`
        // in EDS 3.52 — which reads back as encryption the account does not have,
        // and a transport that tries TLS against the mock server does not connect at
        // all.
        let account = Source::new("jmap-account-plaintext");
        account.set_host("localhost");
        account.set_port(8080);
        account.set_user("vera");
        account.set_secure(false);
        let transport = Source::service(E_SOURCE_EXTENSION_MAIL_TRANSPORT, MAIL_BACKEND_NAME);

        // SAFETY: two live sources.
        unsafe { follow_collection(account.0, transport.0) };

        assert_eq!(
            transport.camel_security_method(),
            CAMEL_NETWORK_SECURITY_METHOD_NONE,
        );
        assert_eq!(
            transport.server(),
            Ok(ServerConfig {
                target: ConnectTarget::Origin("http://localhost:8080".to_owned()),
                user: Some("vera".to_owned()),
            })
        );
    });
}

#[test]
fn an_identity_is_left_without_a_server() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // It reaches none, and a group written on it would be one
        // `e_source_get_extension` created in a source belonging to another part of
        // Evolution — which is the rule `child_added` applies to every child that is
        // not one of this account's own mail services.
        let account = Source::account();
        let identity = Source::identity();

        // SAFETY: two live sources.
        unsafe { follow_collection(account.0, identity.0) };

        assert!(!identity.has(E_SOURCE_EXTENSION_AUTHENTICATION));
        assert!(!identity.has(E_SOURCE_EXTENSION_SECURITY));
    });
}

#[test]
fn a_mail_source_of_another_provider_is_left_alone() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let account = Source::account();
        let smtp = Source::service(E_SOURCE_EXTENSION_MAIL_TRANSPORT, c"smtp");

        // SAFETY: two live sources.
        unsafe { follow_collection(account.0, smtp.0) };

        assert!(
            !smtp.has(E_SOURCE_EXTENSION_AUTHENTICATION),
            "another provider's service was pointed at this account's server"
        );
        assert!(!smtp.has(E_SOURCE_EXTENSION_SECURITY));
    });
}

#[test]
fn an_account_with_no_security_group_reaches_its_mail_sources_as_tls() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // A hand-written `.source` file — the manual test recipe's — has no
        // `[Security]` group, and `collection_source::server_of` reads that as TLS
        // rather than as "the user switched encryption off". A mail source left with
        // no `[Security]` at all would instead take `CamelNetworkSettings`' own
        // default, which is a third answer nobody chose.
        let account = Source::new("jmap-account-no-security");
        account.set_host("jmap.example.com");
        let transport = Source::service(E_SOURCE_EXTENSION_MAIL_TRANSPORT, MAIL_BACKEND_NAME);

        // SAFETY: two live sources.
        unsafe { follow_collection(account.0, transport.0) };

        assert_eq!(
            transport.security_method().as_deref(),
            Some("ssl-on-alternate-port")
        );
        assert!(
            !account.has(E_SOURCE_EXTENSION_SECURITY),
            "reading the account added a group to the user's own file"
        );
    });
}

#[test]
fn an_account_that_names_no_server_writes_none_on_its_mail_sources() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Nothing to carry, and the group is left absent on both: on the account
        // because it is the user's file, and on the mail source because a host this
        // backend does not know is not one to invent.
        let account = Source::new("jmap-account-no-server");
        account.set_secure(true);
        let transport = Source::service(E_SOURCE_EXTENSION_MAIL_TRANSPORT, MAIL_BACKEND_NAME);

        // SAFETY: two live sources.
        unsafe { follow_collection(account.0, transport.0) };

        assert!(!transport.has(E_SOURCE_EXTENSION_AUTHENTICATION));
        assert!(
            !account.has(E_SOURCE_EXTENSION_AUTHENTICATION),
            "reading the account added a group to the user's own file"
        );
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_mail_child_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}
