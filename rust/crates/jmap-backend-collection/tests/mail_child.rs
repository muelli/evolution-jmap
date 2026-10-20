// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests that the mail sources of an account follow the account.
// `tests/child_added.rs` does not cover this case, because this backend does not
// create the mail sources.
//
// These tests assert two things that no other test can assert. First, the
// transport gets a server. The assistant creates the transport and the setup
// module writes only its service name: the sending page is hidden for a
// store-and-transport provider. Only this backend has both the account and its
// mail children.
//
// Second, `[Security] Method` on a mail source must be a Camel enum nick. A plain
// `secure` binding writes `"tls"`, which Camel does not know. The store then
// keeps the `CamelNetworkSettings` default security method.
//
// The sources are real `ESource`s, and the tests change the account through the
// EDS setters. Thus the tests assert a live `GBinding`, not a copy.

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
// The token-cache-key accessors that the installed EDS has. See
// `jmap_backend_collection::child_added::CREDENTIAL_PROPERTY`.
#[cfg(not(eds_credential_store_id))]
use eds_sys::{
    e_source_authentication_get_credential_name as get_credential_name,
    e_source_authentication_set_credential_name as set_credential_name,
};
#[cfg(eds_credential_store_id)]
use eds_sys::{
    e_source_authentication_get_credential_store_id as get_credential_name,
    e_source_authentication_set_credential_store_id as set_credential_name,
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

/// Generates the `ESourceCamel` subtype for `jmap` and returns its extension
/// name, `[JMAP Backend]`.
///
/// This helper is the same as the one in `jmap-config`'s `tests/mail.rs`. The
/// provider is linked into the test, not installed, so nothing calls
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

/// An `ESource` that the test holds one reference to.
struct Source(*mut ESource);

impl Source {
    fn new(uid: &str) -> Self {
        // SAFETY: no arguments. The types must be registered, because
        // `e_source_get_extension` and `e_source_has_extension` cannot find an
        // extension class whose type nothing has referenced.
        unsafe {
            e_source_authentication_get_type();
            e_source_security_get_type();
            e_source_mail_account_get_type();
            e_source_mail_identity_get_type();
            e_source_mail_transport_get_type();
        }
        let uid = CString::new(uid).expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: the documented arguments: a NUL-terminated uid, the default
        // main context and a GError out-parameter.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    /// The account as `jmap_config::account::apply` writes it: a server, a user,
    /// the credentials-provider method for the password, and TLS.
    fn account() -> Self {
        let source = Self::new("jmap-account");
        source.set_host("jmap.example.com");
        source.set_port(8443);
        source.set_user("vera@example.com");
        source.set_auth_method("plain/password");
        source.set_secure(true);
        source
    }

    /// A mail service source as the assistant and the setup module leave it: a
    /// service name, with no `[Authentication]` and no `[Security]`. The
    /// transport arrives at this backend in this state.
    fn service(extension: &CStr, backend_name: &CStr) -> Self {
        let source = Self::new("jmap-mail-service");
        // SAFETY: a live source and a header constant. The source creates and
        // owns the extension, and the setter copies the string.
        unsafe {
            let backend: *mut ESourceBackend =
                e_source_get_extension(source.0, extension.as_ptr()).cast();
            e_source_backend_set_backend_name(backend, backend_name.as_ptr());
        }
        source
    }

    /// The mail identity source, which names no service because it is a person.
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

    /// `[Authentication]`, created if absent. The test writes the account, so
    /// creating the group is correct here.
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

    fn set_credential_name(&self, name: &str) {
        let name = CString::new(name).expect("no NUL in a literal");
        // SAFETY: as above.
        unsafe { set_credential_name(self.authentication(), name.as_ptr()) };
    }

    fn credential_name(&self) -> Option<String> {
        // SAFETY: as above.
        unsafe { read_string(get_credential_name(self.authentication())) }
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

    /// The `CamelSettings` object that a Camel service gets from this source.
    /// This does the two steps of `e_source_camel_configure_service` without the
    /// service.
    ///
    /// `ESourceCamel` binds host, port, user and security method to
    /// `[Authentication]` and `[Security]` on the same source. Thus this reader
    /// proves that the module wrote those groups.
    fn camel_settings(&self) -> *mut CamelSettings {
        // SAFETY: a live source and the interned extension name of a registered
        // `ESourceCamel` subtype. The source creates and owns the extension and
        // its settings object.
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
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: this holds the only reference.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// The two extensions that name a service and that the module writes on: the
/// receiving account and the sending transport.
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

        // The identity is a person, not a service. If the module treated it as a
        // service, it would get an `[Authentication]` group. Then
        // `collection_backend_child_is_mail` would show a second receiving account.
        let identity = Source::identity();
        // SAFETY: a live source.
        assert_eq!(unsafe { mail_service_of(identity.0) }, None);

        // A service of a different provider. An account can send through its own
        // server with its own password (see
        // `e_util_can_use_collection_as_credential_source`). Writing the collection
        // host on that service would break both.
        let smtp = Source::service(E_SOURCE_EXTENSION_MAIL_TRANSPORT, c"smtp");
        // SAFETY: a live source.
        assert_eq!(unsafe { mail_service_of(smtp.0) }, None);
    });
}

#[test]
fn the_mail_sources_the_assistant_leaves_behind_reach_the_accounts_server() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The assistant creates the three scratch sources and writes the service
        // name. The setup module's `commit_changes` writes the server only on the
        // receiving source, because the sending page is hidden for a
        // store-and-transport provider. Thus the transport names `jmap` and no host.
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
fn a_mail_sources_credential_name_follows_the_account_too() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The token-cache key of eds#663. A mail source must find its OAuth2 token
        // under the key of the account, not under a key of its own.
        let account = Source::account();
        account.set_credential_name("jmap-account");

        for extension in SERVICES {
            let service = Source::service(extension, MAIL_BACKEND_NAME);

            // SAFETY: two live sources.
            unsafe { follow_collection(account.0, service.0) };

            assert_eq!(
                service.credential_name().as_deref(),
                Some("jmap-account"),
                "{extension:?} looks its OAuth2 token up under a different key than its account"
            );
        }
    });
}

#[test]
fn a_mail_source_carries_camels_own_spelling_of_the_security_method() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The EDS `secure` boolean writes a word that Camel does not know. Thus the
        // module cannot bind `secure` on a mail source as `child_added` does on an
        // address book. This assertion pins the EDS spelling.
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

        // Compare with the literal, not the constant. A constant compared with
        // itself does not detect a change to its string.
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
        // `jmap_config::mail::apply_server` writes the Camel nick on the mail
        // account source. A `secure` binding would replace it with the EDS `"tls"`
        // when `G_BINDING_SYNC_CREATE` fires.
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
        // A mail source that requests encryption from a plaintext server (for
        // example, the mock server) does not connect.
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
        // A binding, not a copy: nothing runs the commit again, so a copy keeps the
        // old server. EDS also compares these two host strings to decide if the
        // mail source shares the account password. An old host causes a second
        // password prompt.
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
        // `[Authentication] Method` follows the account, as host, port and user do.
        // `jmap-mail` reads it through `CamelNetworkSettings:auth-mechanism` to
        // select the credential type. `uses_api_token` and `uses_oauth2` use it to
        // select Basic, Bearer or OAuth 2.0. Without it, a mail source always
        // authenticates as Basic, and a Bearer transport asks for a password
        // continuously.
        let account = Source::account();
        let transport = Source::service(E_SOURCE_EXTENSION_MAIL_TRANSPORT, MAIL_BACKEND_NAME);

        // SAFETY: two live sources.
        unsafe { follow_collection(account.0, transport.0) };

        // The binding syncs on creation and on each change, so the Bearer method
        // of the account gets to the transport.
        account.set_auth_method("bearer");
        assert_eq!(
            transport.auth_method().as_deref(),
            Some("bearer"),
            "the transport did not follow the account's Bearer authentication method"
        );

        // A change back to the password method also propagates.
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
        // This test checks the end result, not the written keys. The settings that
        // `e_source_camel_configure_service` gives a `CamelJmapTransport` must name
        // the account server. The test reads them with the `jmap-mail` reader that
        // `connect_sync` calls.
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
        // A wrong `[Security] Method` spelling is visible in this case. An unknown
        // nick keeps the `CamelNetworkSettings` default (`STARTTLS_ON_STANDARD_PORT`
        // in EDS 3.52), which is encryption that the account does not have. A
        // transport that tries TLS with the mock server does not connect.
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
        // An identity has no server. A group written on it would change a source
        // that belongs to a different part of Evolution. `child_added` applies this
        // rule to each child that is not a mail service of this account.
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
        // A hand-written `.source` file (for example, from the manual test recipe)
        // has no `[Security]` group. `collection_source::server_of` reads that as
        // TLS. A mail source with no `[Security]` would get the
        // `CamelNetworkSettings` default, which nobody selected.
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
        // Neither source gets the group. The account is the file of the user, and
        // the backend must not invent a host that it does not know.
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
