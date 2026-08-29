// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// A child following its account, against real `ESource`s — the half of
// `tests/child_source.rs` that happens *after* the child was written.
//
// `apply` copies the account's connection onto a child once, at the populate
// that creates it. Every test in `tests/child_source.rs` is about that copy
// being right; none of them is about what happens when the account changes
// afterwards, which is the case this file is for. The failure it describes has
// no symptom to look at: the child still names a host, still connects, still
// authenticates — at the *old* server, with the old user, or without the TLS the
// account has since been given.
//
// The account and the child here are the real objects, and the account is
// changed through EDS's own setters, so what is asserted is a live `GBinding`
// doing its work rather than a second copy taken by this test.

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

/// An `ESource` in the state EDS hands one back in — a uid and nothing else —
/// with the extensions each test puts on it.
struct Source(*mut ESource);

impl Source {
    fn new(uid: &str) -> Self {
        let uid = CString::new(uid).expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a GError
        // out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    /// The account source, as a user's `.source` file describes it: the four
    /// `[Authentication]` fields and the `[Security]` flag, which together are
    /// the whole of what a child copies.
    fn account() -> Self {
        let source = Self::new("jmap-account");
        source.set_host("jmap.example.com");
        source.set_port(8443);
        source.set_user("vera@example.com");
        source.set_auth_method("plain/password");
        source.set_secure(true);
        source
    }

    /// A child of that account, written the way a populate writes one — through
    /// `child_source::apply`, so the properties are the ones this backend really
    /// puts on a child rather than ones this test chose.
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
        // SAFETY: a live source, a header constant, and the extension is
        // present, so this returns the source's own.
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

    fn set_secure(&self, secure: bool) {
        // The `[Security]` extension of a source that has none yet: created
        // here, deliberately, because this is a test building an account rather
        // than the backend reading one.
        // SAFETY: a live source and a header constant; the extension is created
        // on demand and owned by the source.
        let security: *mut ESourceSecurity =
            unsafe { e_source_get_extension(self.0, E_SOURCE_EXTENSION_SECURITY.as_ptr()).cast() };
        // SAFETY: a live extension.
        unsafe { e_source_security_set_secure(security, if secure { GTRUE } else { GFALSE }) };
    }

    /// `[Authentication]`, created on demand — as for [`Source::set_secure`],
    /// this is the test writing an account, not the backend reading one.
    fn authentication(&self) -> *mut ESourceAuthentication {
        // SAFETY: a live source and a header constant.
        unsafe { e_source_get_extension(self.0, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast() }
    }

    fn host(&self) -> Option<String> {
        let auth: *mut ESourceAuthentication = self.extension(E_SOURCE_EXTENSION_AUTHENTICATION);
        // SAFETY: a live extension; the string is owned by it.
        unsafe { read_string(e_source_authentication_get_host(auth)) }
    }

    /// The child as the address book and calendar backends read it — origin,
    /// user and resource id in one, which is what a stale child gets wrong.
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

/// The account and one of its children, bound — the state every test below
/// starts from, and the one `child_added` leaves behind.
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
        // A property name is a string on this call, so a misspelling is not a
        // compile error — it is a `g_critical` at runtime and a binding that was
        // never made, which is exactly the silent staleness this module exists to
        // remove. Asked of GObject rather than assumed, and asked of the *class*, so
        // it fails whether or not any other test happens to exercise that property.
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
                // SAFETY: a live GObject; `g_object_class_find_property` answers
                // NULL for a name the class does not carry.
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
        // The case the binding exists for, and the one no copy at populate can
        // reach: a child written by an earlier session, loaded from the backend's
        // cache directory on this one, carrying the server the account named *then*.
        // `child_added` fires for it before anything connects, so this is where the
        // account's current answer has to reach it.
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
            },
            "the child is still asking last session's server"
        );
    });
}

#[test]
fn moving_the_account_to_another_server_moves_its_children() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The user edits the account: a new host, a new port. Nothing re-runs
        // `apply` — a populate only writes children it creates — so without the
        // binding the child goes on reaching the old server.
        let (account, child) = bound();

        account.set_host("jmap.example.org");
        account.set_port(443);

        assert_eq!(
            child.config().target,
            // A stated default port serializes out of the origin (RFC 6454 §6.2);
            // the child still follows the renamed server either way.
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
        // The one change that a child getting it wrong is a *security* difference
        // rather than a connection failure — in both directions. A child left at
        // "tls" against an account switched to "none" fails visibly; the reverse,
        // which is this assertion's other half below, does not.
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
        // Not a credential — how EDS is to *obtain* one. A child that disagrees with
        // its account is a child prompted for a password the account does not use.
        let (account, child) = bound();

        account.set_auth_method("OAuth2");

        let auth: *mut ESourceAuthentication = child.extension(E_SOURCE_EXTENSION_AUTHENTICATION);
        // SAFETY: a live extension; the string is owned by it.
        let method = unsafe { read_string(eds_sys::e_source_authentication_get_method(auth)) };
        assert_eq!(method.as_deref(), Some("OAuth2"));
    });
}

#[test]
fn a_child_does_not_write_back_to_the_account() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // One-way, deliberately: the child sources are this backend's to write, and
        // a binding that carried a child's value back would let one of them edit the
        // user's account file — and, through the account, every other child.
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
        // The rule that keeps this out of the user's file: `e_source_get_extension`
        // creates what it cannot find, and the account source is the one thing this
        // backend must only ever read. An account with no `[Authentication]` names
        // no host, so there is nothing to carry anyway.
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
        // `child_added` fires for every source parented to the collection, mail
        // identities included, and this backend writes only some of them. A source
        // with no `[Authentication]` connects to nothing, so binding one onto it
        // would be this backend editing a source belonging to another part of
        // Evolution.
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
        // The two groups are decided separately, and this is the pair that says so:
        // a child with `[Authentication]` and no `[Security]` — which is what a mail
        // account source created by the setup UI can be — follows the account's host
        // and user without being given a `[Security]` group it never had.
        let account = Source::account();
        let mail = Source::new("jmap-account-mail");
        mail.set_host("stale.example.com");
        // The address book children this backend writes always carry both groups, so
        // reaching this state at all takes a source it did not write.
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
// The `[JMAP OAuth2]` group — bound by its own rule, because EDS hands the
// OAuth2 service whichever source asked for the token.
//
// `e_source_registry_server_get_access_token_sync` (e-source-registry-server.c,
// EDS 3.52) passes the ASKING source straight into
// `e_oauth2_service_get_access_token_sync` — no credential-source resolution to
// the collection happens on the silent-refresh path (only the interactive
// prompter resolves it). EDS's own services never notice: their client ids are
// compile-time constants. Ours lives in `[JMAP OAuth2]`, and when only the
// account carried it, any child-context refresh ran with no client id, no
// token endpoint and no scope — so the first expired access token turned into
// a full re-consent instead of a silent refresh. Observed live 2026-08-26:
// the registry prepared a refresh-token form for a memory-only calendar
// child's uid, and the shell escalated to the consent window.

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

        // And it is a live binding, not a copy: a re-registration that lands on
        // the account reaches every child that was already bound.
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
    // The transport is the child whose missing registration the operator
    // actually saw: Send popped the consent window (roadmap item 15). It takes
    // the `follow_server` early path in `follow_collection`, so the OAuth2
    // binding must happen before that fork.
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let account = Source::account();
        // SAFETY: a live source and a config of literals.
        unsafe { jmap_config::oauth2::apply(account.0, &oauth2_config()) };

        let transport = Source::new("jmap-account-transport");
        // SAFETY: a live source and a header constant; the extension is
        // created on demand and owned by the source, and both mail extension
        // types derive from ESourceBackend.
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
    // The rule the rest of this file lives by still holds where it matters:
    // nothing invents the group on the ACCOUNT, whose `.source` file is the
    // user's; and a child of an account that has no client registration gains
    // an empty group nothing will read.
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
// `GBinding` lifetime — what a live binding does when one of the two
// `ESource`s it joins is finalized.
//
// `e_binding_bind_property` is `camel_binding_bind_property`
// (EDS 3.52.3's `src/camel/camel.c`): a `GRecMutex` around plain
// `g_object_bind_property`. GLib 2.80's own `GBinding` (`gobject/gbinding.c`)
// installs a `g_object_weak_ref` on both endpoints at construction and drops
// its own sole reference exactly once, the first time either fires — so the
// binding never outlives the shorter-lived of the two extensions it joins,
// and never touches the one that is already gone. That is GLib's contract,
// not this module's — `follow_collection`'s and `follow_server`'s own Safety
// comments both assert it, correctly, but neither was pinned by a test that
// actually finalizes one side of a live binding and keeps using the other,
// which is the thing a future change here (e.g. holding onto a raw extension
// pointer past the call) could get wrong without GLib's guarantee helping at
// all.
//
// Finalizing an `ESource` is *not* one plain `g_object_unref` away, which the
// first test below exists to establish before the rest rely on it: every
// `E_SOURCE_PARAM_SETTING` property write (which is what an account edit, and
// this module's own `bind()` with `G_BINDING_SYNC_CREATE`, both are) calls
// `e_source_changed()`, which schedules a debounced idle callback holding its
// own `g_object_ref` on the source — released only when that idle fires in
// whatever `GMainContext` the source uses (the default one here, since these
// tests build sources with no explicit context). A production process has a
// `GMainLoop` permanently iterating that context, so the idle fires within
// one turn and the extra ref is gone almost as soon as it was taken; a bare
// unit test does not run one at all, so without pumping the context by hand
// the source this file's tests drop never actually finalizes — every
// assertion after it would then be true for the wrong reason (nothing was
// ever freed) rather than for the reason each test names.
fn pump_pending_idle_changed() {
    // SAFETY: `NULL` is the documented way to name the default `GMainContext`
    // (the one these tests' sources use); `FALSE` never blocks, so a source
    // with no more idle callbacks pending simply stops early.
    for _ in 0..16 {
        if unsafe { glib_sys::g_main_context_iteration(ptr::null_mut(), GFALSE) } == GFALSE {
            break;
        }
    }
}

unsafe extern "C" fn mark_finalized(user_data: gpointer, _object: *mut gobject_sys::GObject) {
    // SAFETY: `user_data` is the `&'static AtomicBool` the test below passed
    // to `g_object_weak_ref`, which hands it back unchanged.
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
        // SAFETY: `auth` is a live GObject; `finalized` outlives the weak
        // ref, which never outlives this test's `source`.
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

        // If the binding's weak-unref on the child (the target) had not
        // disconnected the notify handler it installed on the account (the
        // source), this write would run the binding's transform into a
        // freed extension instead of doing nothing.
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
        // If `follow_oauth2`'s binding onto the (now-freed) child's extension
        // had survived, this apply's `notify` would run the binding into it
        // instead of doing nothing.
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
