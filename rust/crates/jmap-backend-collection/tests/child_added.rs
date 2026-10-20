// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests that a child follows its account, with real `ESource`s.
//
// `apply` copies the account connection to a child once, when the populate
// creates the child. `tests/child_source.rs` tests that copy. This file tests
// what occurs when the account changes after the copy. That failure has no
// visible symptom: the child connects, but to the old server, with the old user,
// or without TLS.
//
// The tests change the account through the EDS setters. Thus the tests assert a
// live `GBinding`, not a copy.

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use eds_sys::{
    E_SOURCE_EXTENSION_ADDRESS_BOOK, E_SOURCE_EXTENSION_AUTHENTICATION,
    E_SOURCE_EXTENSION_SECURITY, ESource, ESourceAuthentication, ESourceSecurity,
    e_source_authentication_get_host, e_source_authentication_get_type,
    e_source_authentication_set_host, e_source_authentication_set_method,
    e_source_authentication_set_port, e_source_authentication_set_user, e_source_get_extension,
    e_source_has_extension, e_source_new_with_uid, e_source_security_get_type,
    e_source_security_set_secure,
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
use glib_sys::{GFALSE, GTRUE, gpointer};
use gobject_sys::{g_object_unref, g_object_weak_ref};
use jmap_backend_collection::child_added::{BOUND, follow_collection};
use jmap_backend_collection::child_source::apply;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::source::{ConnectTarget, SourceConfig};
use jmap_collection_sync::child_source::Connection;
use jmap_collection_sync::{Child, ChildKind};
use jmap_proto::Id;

mod common;
use common::{with_timeout, with_timeout_duration};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// An `ESource` that starts with a uid and no extensions.
struct Source(*mut ESource);

impl Source {
    fn new(uid: &str) -> Self {
        let uid = CString::new(uid).expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: the documented arguments: a NUL-terminated uid, the default
        // main context and a GError out-parameter.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    /// The account source as a `.source` file of the user describes it: the
    /// four `[Authentication]` fields and the `[Security]` flag. A child copies
    /// all of these and nothing more.
    fn account() -> Self {
        let source = Self::new("jmap-account");
        source.set_host("jmap.example.com");
        source.set_port(8443);
        source.set_user("vera@example.com");
        source.set_auth_method("plain/password");
        source.set_secure(true);
        source
    }

    /// A child of the account, written through `child_source::apply` as a
    /// populate writes it. Thus the child has the properties that the backend
    /// writes.
    fn child_of(connection: &Connection) -> Self {
        let source = Self::new("jmap-account-child");
        let child = Child {
            resource_id: ChildKind::AddressBook.resource_id(&Id::new("AB1")),
            kind: ChildKind::AddressBook,
            display_name: "Personal".to_owned(),
            account_id: Id::new("A1"),
            collection_id: Id::new("AB1"),
            is_default: false,
            color: None,
            read_only: false,
            remote_deletable: true,
        };
        // SAFETY: a live source.
        unsafe { apply(source.0, &child.settings(connection)) }
            .expect("a child this backend wrote");
        source
    }

    fn extension<T>(&self, name: &CStr) -> *mut T {
        assert!(
            self.has(name),
            "the test would have created the extension it meant to read"
        );
        // SAFETY: a live source and a header constant. The extension is present,
        // so the call returns the existing extension of the source.
        unsafe { e_source_get_extension(self.0, name.as_ptr()) }.cast()
    }

    fn has(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a NUL-terminated name.
        unsafe { e_source_has_extension(self.0, name.as_ptr()) != GFALSE }
    }

    fn set_host(&self, host: &str) {
        let host = CString::new(host).expect("no NUL in a literal");
        let auth: *mut ESourceAuthentication = self.authentication();
        // SAFETY: a live extension; the setter copies the string.
        unsafe { e_source_authentication_set_host(auth, host.as_ptr()) };
    }

    fn set_port(&self, port: u16) {
        let auth: *mut ESourceAuthentication = self.authentication();
        // SAFETY: a live extension.
        unsafe { e_source_authentication_set_port(auth, port) };
    }

    fn set_user(&self, user: &str) {
        let user = CString::new(user).expect("no NUL in a literal");
        let auth: *mut ESourceAuthentication = self.authentication();
        // SAFETY: as above.
        unsafe { e_source_authentication_set_user(auth, user.as_ptr()) };
    }

    fn set_auth_method(&self, method: &str) {
        let method = CString::new(method).expect("no NUL in a literal");
        let auth: *mut ESourceAuthentication = self.authentication();
        // SAFETY: as above.
        unsafe { e_source_authentication_set_method(auth, method.as_ptr()) };
    }

    fn set_credential_name(&self, name: &str) {
        let name = CString::new(name).expect("no NUL in a literal");
        let auth: *mut ESourceAuthentication = self.authentication();
        // SAFETY: as above.
        unsafe { set_credential_name(auth, name.as_ptr()) };
    }

    fn credential_name(&self) -> Option<String> {
        let auth: *mut ESourceAuthentication = self.extension(E_SOURCE_EXTENSION_AUTHENTICATION);
        // SAFETY: a live extension; the string is owned by it.
        unsafe { read_string(get_credential_name(auth)) }
    }

    fn set_secure(&self, secure: bool) {
        // The test writes the account, so creating `[Security]` here is correct.
        // SAFETY: a live source and a header constant. The source creates and
        // owns the extension.
        let security: *mut ESourceSecurity =
            unsafe { e_source_get_extension(self.0, E_SOURCE_EXTENSION_SECURITY.as_ptr()).cast() };
        // SAFETY: a live extension.
        unsafe { e_source_security_set_secure(security, if secure { GTRUE } else { GFALSE }) };
    }

    /// `[Authentication]`, created if absent. As in [`Source::set_secure`], the
    /// test writes the account, so creating the group is correct.
    fn authentication(&self) -> *mut ESourceAuthentication {
        // SAFETY: a live source and a header constant.
        unsafe { e_source_get_extension(self.0, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast() }
    }

    fn host(&self) -> Option<String> {
        let auth: *mut ESourceAuthentication = self.extension(E_SOURCE_EXTENSION_AUTHENTICATION);
        // SAFETY: a live extension; the string is owned by it.
        unsafe { read_string(e_source_authentication_get_host(auth)) }
    }

    /// The child as the address book and calendar backends read it: origin,
    /// user and resource id.
    fn config(&self) -> SourceConfig {
        // SAFETY: a live source.
        unsafe { SourceConfig::from_source(self.0) }.expect("a child names its account's server")
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: this holds the only reference.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

fn connection() -> Connection {
    Connection {
        host: "jmap.example.com".to_owned(),
        port: Some(8443),
        user: Some("vera@example.com".to_owned()),
        auth_method: Some("plain/password".to_owned()),
        secure: true,
    }
}

/// The account and one bound child, as `child_added` leaves them.
fn bound() -> (Source, Source) {
    let account = Source::account();
    let child = Source::child_of(&connection());
    // SAFETY: two live sources.
    unsafe { follow_collection(account.0, child.0) };
    (account, child)
}

#[test]
fn the_bound_properties_exist_on_the_extensions_they_are_named_under() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // A misspelt property name is not a compile error. It causes a runtime
        // `g_critical` and no binding. The test asks the GObject class, so it fails
        // even if no other test uses that property.
        // SAFETY: no arguments; the type system initialises itself.
        unsafe {
            e_source_authentication_get_type();
            e_source_security_get_type();
        }

        for (extension, properties) in BOUND {
            let source = Source::new("jmap-property-check");
            // SAFETY: a live source and a header constant.
            let object = unsafe { e_source_get_extension(source.0, extension.as_ptr()) };
            assert!(!object.is_null(), "{extension:?} is not an extension");

            for property in properties {
                // SAFETY: a live GObject. `g_object_class_find_property` returns
                // NULL for a name that the class does not have.
                let found = unsafe {
                    let class = (*object.cast::<gobject_sys::GObject>())
                        .g_type_instance
                        .g_class;
                    gobject_sys::g_object_class_find_property(class.cast(), property.as_ptr())
                };
                assert!(
                    !found.is_null(),
                    "{extension:?} has no property called {property:?}"
                );
            }
        }
    });
}

#[test]
fn a_cached_child_is_brought_up_to_date_the_moment_it_is_bound() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // An earlier session wrote this child, and this session loads it from the
        // cache directory with the old server. A copy at populate cannot update it.
        // `child_added` fires before a connection, so the binding updates it here.
        let account = Source::account();
        let stale = Connection {
            host: "old.example.com".to_owned(),
            port: Some(1234),
            user: Some("someone-else@example.com".to_owned()),
            ..connection()
        };
        let child = Source::child_of(&stale);
        assert_eq!(
            child.config(),
            SourceConfig {
                target: ConnectTarget::Origin("https://old.example.com:1234".to_owned()),
                user: Some("someone-else@example.com".to_owned()),
                resource_id: Some("AB1".to_owned()),
                rebase_urls: false,
            },
            "the test did not start from a stale child"
        );

        // SAFETY: two live sources.
        unsafe { follow_collection(account.0, child.0) };

        assert_eq!(
            child.config(),
            SourceConfig {
                target: ConnectTarget::Origin("https://jmap.example.com:8443".to_owned()),
                user: Some("vera@example.com".to_owned()),
                resource_id: Some("AB1".to_owned()),
                rebase_urls: false,
            },
            "the child is still asking last session's server"
        );
    });
}

#[test]
fn moving_the_account_to_another_server_moves_its_children() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The user changes the host and the port. A populate writes only the
        // children that it creates, so nothing runs `apply` again. Without the
        // binding, the child keeps the old server.
        let (account, child) = bound();

        account.set_host("jmap.example.org");
        account.set_port(443);

        assert_eq!(
            child.config().target,
            // The origin omits a default port (RFC 6454 §6.2).
            ConnectTarget::Origin("https://jmap.example.org".into()),
            "the child stayed with the server the account used to name"
        );
    });
}

#[test]
fn renaming_the_account_user_renames_its_children() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (account, child) = bound();

        account.set_user("vera@example.org");

        assert_eq!(
            child.config().user,
            Some("vera@example.org".to_owned()),
            "the child would authenticate as somebody the account no longer is"
        );
    });
}

#[test]
fn switching_tls_off_on_the_account_reaches_its_children() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // An error here is a security problem, not only a connection failure. A
        // child on TLS with a plaintext account fails visibly. A child on plain
        // text with a TLS account (the second half) does not.
        let (account, child) = bound();

        account.set_secure(false);
        account.set_host("localhost");
        assert_eq!(
            child.config().target,
            ConnectTarget::Origin("http://localhost:8443".into()),
            "the child still believes the account is on TLS"
        );

        account.set_secure(true);
        assert_eq!(
            child.config().target,
            ConnectTarget::Origin("https://localhost:8443".into()),
            "the child stayed on plain text after the account went back to TLS"
        );
    });
}

#[test]
fn the_authentication_method_follows_too() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The method tells EDS how to get a credential. A child with a different
        // method asks for a password that the account does not use.
        let (account, child) = bound();

        account.set_auth_method("OAuth2");

        let auth: *mut ESourceAuthentication = child.extension(E_SOURCE_EXTENSION_AUTHENTICATION);
        // SAFETY: a live extension; the string is owned by it.
        let method = unsafe { read_string(eds_sys::e_source_authentication_get_method(auth)) };
        assert_eq!(method.as_deref(), Some("OAuth2"));
    });
}

#[test]
fn the_credential_name_follows_too() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // eds#663: EDS stores the OAuth2 credentials of an account under the uid
        // of the account. A child must use that key, not its own key or no key.
        let (account, child) = bound();

        account.set_credential_name("jmap-account");

        assert_eq!(
            child.credential_name().as_deref(),
            Some("jmap-account"),
            "the child looks its OAuth2 token up under a different key than its account"
        );
    });
}

#[test]
fn a_child_does_not_write_back_to_the_account() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The binding is one-way. A two-way binding would let a child change the
        // account file of the user, and through it, all the other children.
        let (account, child) = bound();

        child.set_host("somewhere-else.example.com");

        assert_eq!(
            account.host().as_deref(),
            Some("jmap.example.com"),
            "a child rewrote the account"
        );
    });
}

#[test]
fn an_account_with_no_authentication_group_is_not_given_one() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `e_source_get_extension` creates a missing extension, but the backend must
        // only read the account source. An account with no `[Authentication]` names
        // no host, so there is nothing to bind.
        let account = Source::new("jmap-account-without-authentication");
        let child = Source::child_of(&connection());

        // SAFETY: two live sources.
        unsafe { follow_collection(account.0, child.0) };

        assert!(
            !account.has(E_SOURCE_EXTENSION_AUTHENTICATION),
            "the account was given an [Authentication] group it did not have"
        );
        assert!(
            !account.has(E_SOURCE_EXTENSION_SECURITY),
            "the account was given a [Security] group it did not have"
        );
        assert_eq!(
            child.config().target,
            ConnectTarget::Origin("https://jmap.example.com:8443".into()),
            "the child was reset from an account that says nothing"
        );
    });
}

#[test]
fn a_child_of_a_kind_this_backend_did_not_write_is_left_alone() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `child_added` fires for each child of the collection, mail identities
        // included. A source with no `[Authentication]` connects to nothing. A
        // binding on it would change a source of a different part of Evolution.
        let account = Source::account();
        let identity = Source::new("jmap-account-identity");

        // SAFETY: two live sources.
        unsafe { follow_collection(account.0, identity.0) };

        assert!(
            !identity.has(E_SOURCE_EXTENSION_AUTHENTICATION),
            "a source that authenticates to nothing was given an [Authentication] group"
        );
        assert!(
            !identity.has(E_SOURCE_EXTENSION_SECURITY),
            "a source that connects to nothing was given a [Security] group"
        );
    });
}

#[test]
fn a_child_that_authenticates_but_names_no_security_keeps_naming_none() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The module binds each group separately. A child with `[Authentication]`
        // and no `[Security]` (possible for a mail account from the setup UI)
        // follows the host and the user. It does not get a `[Security]` group.
        let account = Source::account();
        let mail = Source::new("jmap-account-mail");
        mail.set_host("stale.example.com");
        // The backend always writes both groups on an address book child, so this
        // source is not an address book.
        assert!(!mail.has(E_SOURCE_EXTENSION_SECURITY));
        assert!(!mail.has(E_SOURCE_EXTENSION_ADDRESS_BOOK));

        // SAFETY: two live sources.
        unsafe { follow_collection(account.0, mail.0) };

        assert_eq!(
            mail.host().as_deref(),
            Some("jmap.example.com"),
            "the group both sources have was not bound"
        );
        assert!(
            !mail.has(E_SOURCE_EXTENSION_SECURITY),
            "the group only the account has was written onto the child anyway"
        );
    });
}

// ---------------------------------------------------------------------------
// The `[JMAP OAuth2]` group has its own rule, because EDS gives the OAuth2
// service the source that requested the token.
//
// `e_source_registry_server_get_access_token_sync` (e-source-registry-server.c,
// EDS 3.52) passes the requesting source to
// `e_oauth2_service_get_access_token_sync`. On the silent-refresh path, EDS does
// not resolve the credential source to the collection. Only the interactive
// prompter does. The EDS services have compile-time client ids, but this
// project keeps its client id in `[JMAP OAuth2]`. If a child does not have the
// group, a refresh from that child has no client id, token endpoint or scope.
// Then an expired access token causes a full re-consent, not a silent refresh.

fn oauth2_config() -> jmap_config::oauth2::Config {
    jmap_config::oauth2::Config {
        client_id: Some("client-abc123".to_owned()),
        client_secret: None,
        authorization_endpoint: Some("https://jmap.example.com/authorize".to_owned()),
        token_endpoint: Some("https://jmap.example.com/token".to_owned()),
        redirect_uri: Some("org.example.app:/redirect".to_owned()),
        scope: Some("urn:ietf:params:oauth:scope:mail offline_access".to_owned()),
        resource: Some("https://jmap.example.com/session".to_owned()),
    }
}

#[test]
fn a_childs_jmap_oauth2_group_follows_the_accounts() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let account = Source::account();
        // SAFETY: a live source and a config of literals.
        unsafe { jmap_config::oauth2::apply(account.0, &oauth2_config()) };
        let child = Source::child_of(&connection());

        // SAFETY: two live sources, the account and its child.
        unsafe { follow_collection(account.0, child.0) };

        // SAFETY: a live source.
        let carried = unsafe { jmap_config::oauth2::read(child.0) };
        assert_eq!(
            carried,
            oauth2_config(),
            "the child must carry the account's whole client registration"
        );

        // The binding is live: a new registration on the account gets to each
        // bound child.
        let renewed = jmap_config::oauth2::Config {
            client_id: Some("client-def456".to_owned()),
            ..oauth2_config()
        };
        // SAFETY: a live source.
        unsafe { jmap_config::oauth2::apply(account.0, &renewed) };
        // SAFETY: a live source.
        let followed = unsafe { jmap_config::oauth2::read(child.0) };
        assert_eq!(
            followed.client_id.as_deref(),
            Some("client-def456"),
            "a re-registration on the account must reach the bound child"
        );
    });
}

#[test]
fn a_mail_transport_child_gets_the_jmap_oauth2_group_too() {
    // Without the group, Send on the transport opens the consent window. The
    // transport takes the early `follow_server` path in `follow_collection`, so
    // the OAuth2 binding must occur before that branch.
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let account = Source::account();
        // SAFETY: a live source and a config of literals.
        unsafe { jmap_config::oauth2::apply(account.0, &oauth2_config()) };

        let transport = Source::new("jmap-account-transport");
        // SAFETY: a live source and a header constant. The source creates and
        // owns the extension, and both mail extension types derive from
        // ESourceBackend.
        unsafe {
            let backend: *mut eds_sys::ESourceBackend = e_source_get_extension(
                transport.0,
                eds_sys::E_SOURCE_EXTENSION_MAIL_TRANSPORT.as_ptr(),
            )
            .cast();
            eds_sys::e_source_backend_set_backend_name(backend, c"jmap".as_ptr());
        }

        // SAFETY: two live sources, the account and its transport child.
        unsafe { follow_collection(account.0, transport.0) };

        // SAFETY: a live source.
        let carried = unsafe { jmap_config::oauth2::read(transport.0) };
        assert_eq!(
            carried.client_id.as_deref(),
            Some("client-abc123"),
            "the transport must be able to refresh silently at send time"
        );
        assert_eq!(carried.token_endpoint, oauth2_config().token_endpoint);
    });
}

#[test]
fn an_account_without_the_oauth2_group_leaves_the_child_without_it() {
    // Nothing creates the group on the account, because its `.source` file
    // belongs to the user. A child of an account with no client registration
    // does not get the group either.
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let account = Source::account();
        let child = Source::child_of(&connection());

        // SAFETY: two live sources, the account and its child.
        unsafe { follow_collection(account.0, child.0) };

        assert!(
            !account.has(jmap_config::oauth2::EXTENSION_NAME),
            "nothing may invent [JMAP OAuth2] on the user's account file"
        );
        assert!(
            !child.has(jmap_config::oauth2::EXTENSION_NAME),
            "an account with no registration has nothing to carry to a child"
        );
    });
}

// ---------------------------------------------------------------------------
// `GBinding` lifetime: what a live binding does when one of its two `ESource`s
// is finalized.
//
// `e_binding_bind_property` is `camel_binding_bind_property` (EDS 3.52.3
// `src/camel/camel.c`): a `GRecMutex` around `g_object_bind_property`. The GLib
// 2.80 `GBinding` (`gobject/gbinding.c`) puts a `g_object_weak_ref` on both
// endpoints. It releases its only reference once, when the first weak ref fires.
// Thus the binding does not outlive either extension, and does not touch a
// finalized one. The Safety comments of `follow_collection` and `follow_server`
// rely on this GLib contract, and these tests pin it.
//
// A plain `g_object_unref` does not finalize an `ESource`; the first test
// below proves this. Each `E_SOURCE_PARAM_SETTING` property write (an account
// edit, or `bind()` with `G_BINDING_SYNC_CREATE`) calls `e_source_changed()`.
// That schedules an idle callback which holds a `g_object_ref` on the source.
// The idle releases the reference when it runs in the `GMainContext` of the
// source (here, the default context). A production process runs a `GMainLoop`,
// but a unit test does not. Thus the tests pump the context, or no source is
// finalized and the assertions pass for the wrong reason.
fn pump_pending_idle_changed() {
    // SAFETY: `NULL` names the default `GMainContext`, which the test sources
    // use. `FALSE` does not block, so the loop stops when no idle is pending.
    for _ in 0..16 {
        if unsafe { glib_sys::g_main_context_iteration(ptr::null_mut(), GFALSE) } == GFALSE {
            break;
        }
    }
}

unsafe extern "C" fn mark_finalized(user_data: gpointer, _object: *mut gobject_sys::GObject) {
    // SAFETY: `user_data` is the `&'static AtomicBool` that the test gave to
    // `g_object_weak_ref`, which returns it unchanged.
    let flag = unsafe { &*(user_data as *const AtomicBool) };
    flag.store(true, Ordering::SeqCst);
}

#[test]
fn dropping_a_source_finalizes_its_extension_once_the_pending_idle_runs() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let source = Source::account();
        let auth: *mut ESourceAuthentication = source.authentication();
        let finalized: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
        // SAFETY: `auth` is a live GObject. `finalized` outlives the weak ref,
        // which does not outlive `source`.
        unsafe {
            g_object_weak_ref(
                auth.cast(),
                Some(mark_finalized),
                finalized as *const AtomicBool as gpointer,
            );
        }

        drop(source);
        assert!(
            !finalized.load(Ordering::SeqCst),
            "the extension was finalized before the pending idle ran — the \
             premise this module comment states is wrong, or already fixed"
        );

        pump_pending_idle_changed();
        assert!(
            finalized.load(Ordering::SeqCst),
            "the extension outlived the source that owned it, even once the \
             pending idle had a chance to run"
        );
    });
}

#[test]
fn dropping_the_child_disconnects_the_binding_and_the_account_stays_usable() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (account, child) = bound();

        drop(child);
        pump_pending_idle_changed();

        // The weak ref on the child (the target) must disconnect the notify
        // handler on the account (the source). If not, this write uses a freed
        // extension.
        account.set_host("moved.example.com");

        assert_eq!(
            account.host().as_deref(),
            Some("moved.example.com"),
            "the account itself must still work once its bound child is gone"
        );
    });
}

#[test]
fn dropping_the_account_leaves_the_childs_last_values_in_place() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (account, child) = bound();
        let before = child.config();

        drop(account);
        pump_pending_idle_changed();

        assert_eq!(
            child.config(),
            before,
            "a child must keep the values it was bound with once the account is gone"
        );
    });
}

#[test]
fn dropping_the_child_does_not_break_a_later_oauth2_reregistration() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let account = Source::account();
        // SAFETY: a live source and a config of literals.
        unsafe { jmap_config::oauth2::apply(account.0, &oauth2_config()) };
        let child = Source::child_of(&connection());
        // SAFETY: two live sources.
        unsafe { follow_collection(account.0, child.0) };

        drop(child);
        pump_pending_idle_changed();

        let renewed = jmap_config::oauth2::Config {
            client_id: Some("client-def456".to_owned()),
            ..oauth2_config()
        };
        // If the `follow_oauth2` binding to the freed child extension stayed,
        // the `notify` of this apply would use the freed extension.
        // SAFETY: a live source.
        unsafe { jmap_config::oauth2::apply(account.0, &renewed) };

        // SAFETY: a live source.
        let read_back = unsafe { jmap_config::oauth2::read(account.0) };
        assert_eq!(
            read_back.client_id.as_deref(),
            Some("client-def456"),
            "the account must still take a re-registration once its bound child is gone"
        );
    });
}

#[test]
fn dropping_the_account_leaves_the_childs_oauth2_group_in_place() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let account = Source::account();
        // SAFETY: a live source and a config of literals.
        unsafe { jmap_config::oauth2::apply(account.0, &oauth2_config()) };
        let child = Source::child_of(&connection());
        // SAFETY: two live sources.
        unsafe { follow_collection(account.0, child.0) };

        drop(account);
        pump_pending_idle_changed();

        // SAFETY: a live source.
        let carried = unsafe { jmap_config::oauth2::read(child.0) };
        assert_eq!(
            carried,
            oauth2_config(),
            "a child must keep the registration it was bound with once the account is gone"
        );
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_child_added_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}
