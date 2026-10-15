// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for stale bearer tokens and credential refreshing on pooled connections.

use std::ptr;

use eds_sys::{
    EBookMetaBackend, EBookMetaBackendClass, e_book_meta_backend_info_free, e_client_error_quark,
};
use glib_sys::{GError, GSList, GTRUE, g_error_free, g_free, g_slist_free_full, gchar};
use gobject_sys::{g_type_class_ref, g_type_class_unref};
use jmap_backend_book::backend::{JmapBookBackend, JmapBookBackendClass};
use jmap_backend_core::subclass::register_static;
use jmap_book_sync::BookSync;
use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::contacts::ContactCard;

const GOOD: &str = "token-the-connection-was-built-with";
const FRESH: &str = "token-a-refresh-would-hand-back";

/// The class EDS would dispatch through, kept referenced for the test's
/// duration so the vfunc pointers stay valid.
struct Class(*mut JmapBookBackendClass);

impl Class {
    fn get() -> Self {
        let gtype = register_static::<JmapBookBackend>();
        assert_ne!(gtype, 0, "the backend type did not register");
        // SAFETY: the type is registered, so referencing its class runs
        // class_init and hands back a class struct of our own layout.
        Self(unsafe { g_type_class_ref(gtype) }.cast())
    }

    fn vfuncs(&self) -> &EBookMetaBackendClass {
        // SAFETY: the class is referenced and leads with the parent's.
        unsafe { &(*self.0).parent_class }
    }
}

impl Drop for Class {
    fn drop(&mut self) {
        // SAFETY: the reference taken in `get` is given back exactly once.
        unsafe { g_type_class_unref(self.0.cast()) };
    }
}

/// Detached backend instance for test isolation.
struct Detached(Box<JmapBookBackend>);

impl Detached {
    fn as_ptr(&mut self) -> *mut EBookMetaBackend {
        ptr::from_mut(&mut *self.0).cast()
    }
}

/// What `list_existing_sync` answered, with its out-parameters freed the way
/// EDS frees them.
struct Listed {
    ok: bool,
    error: *mut GError,
    count: usize,
}

impl Listed {
    fn assert_is_an_authentication_failure(&self) {
        assert!(!self.ok, "a stale token was accepted");
        assert!(!self.error.is_null(), "the failure reported no error");
        // SAFETY: a live GError this struct owns.
        unsafe {
            assert_eq!(
                (*self.error).domain,
                e_client_error_quark(),
                "the 401 was reported in the wrong error domain"
            );
            assert_eq!(
                (*self.error).code,
                eds_sys::E_CLIENT_ERROR_AUTHENTICATION_FAILED as i32,
                "a 401 EDS cannot re-authenticate must still ask it to"
            );
        }
    }
}

impl Drop for Listed {
    fn drop(&mut self) {
        // SAFETY: both are what the vfunc handed over, or NULL.
        unsafe {
            if !self.error.is_null() {
                g_error_free(self.error);
            }
        }
    }
}

fn list_existing(class: &Class, backend: &mut Detached) -> Listed {
    let vfunc = class
        .vfuncs()
        .list_existing_sync
        .expect("list_existing_sync is installed");
    let mut tag: *mut gchar = ptr::null_mut();
    let mut objects: *mut GSList = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: valid instance of ours, NULL cancellable, and writable out-parameters.
    let ok = unsafe {
        vfunc(
            backend.as_ptr(),
            &mut tag,
            &mut objects,
            ptr::null_mut(),
            &mut error,
        )
    };

    // SAFETY: out-parameters are transfer-full when the call succeeded, NULL otherwise.
    let count = unsafe {
        let count = glib_sys::g_slist_length(objects) as usize;
        g_slist_free_full(objects, Some(e_book_meta_backend_info_free));
        if !tag.is_null() {
            g_free(tag.cast());
        }
        count
    };

    Listed {
        ok: ok == GTRUE,
        error,
        count,
    }
}

/// A mock that accepts exactly one bearer token at a time, with one contact in
/// its default address book.
struct Fixture {
    server: MockServer,
    account_id: Id,
    book: Id,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().bearer_token(GOOD).start();
        let account_id = server.account_id();
        let book = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            state
                .account_mut(&account_id)
                .unwrap()
                .seed_address_book("Personal", true)
        };
        let fixture = Self {
            server,
            account_id,
            book,
        };
        fixture
            .client(GOOD)
            .contact_create(
                &fixture.account_id,
                &ContactCard::simple(fixture.book.clone(), "Vera Rubin", "vera@example.com"),
            )
            .expect("seeded a contact");
        fixture
    }

    fn client(&self, token: &str) -> Client {
        Client::connect(self.server.origin(), Credentials::bearer(token)).expect("connected")
    }

    fn detached(&self, token: &str) -> Detached {
        let backend = JmapBookBackend::detached();
        backend.store_connection(BookSync::new(
            self.client(token),
            self.account_id.clone(),
            self.book.clone(),
        ));
        Detached(backend)
    }
}

#[test]
fn a_stale_bearer_token_is_reported_once_when_there_is_nothing_to_refresh_from() {
    let class = Class::get();
    let fixture = Fixture::start();
    let mut backend = fixture.detached(GOOD);

    // The connection is good to begin with, so the failure below is the
    // rotation and nothing else.
    let listed = list_existing(&class, &mut backend);
    assert!(listed.ok, "a live token was refused");
    assert_eq!(listed.count, 1);

    // The server now accepts a different token; the pooled connection still
    // carries the old one.
    fixture.server.set_bearer_token(FRESH);
    let before = fixture.server.unauthorized_responses();
    let listed = list_existing(&class, &mut backend);
    listed.assert_is_an_authentication_failure();

    // A detached instance has no ESource, so refresh reports nothing to refresh
    // and does not retry.
    assert_eq!(
        fixture.server.unauthorized_responses() - before,
        1,
        "a 401 with no refreshable credentials must not be retried"
    );
}

#[test]
fn fresh_credentials_installed_on_the_live_connection_fix_the_very_next_vfunc_call() {
    // Replaces credentials on the pooled connection, ensuring next vfunc call succeeds.
    let class = Class::get();
    let fixture = Fixture::start();
    let mut backend = fixture.detached(GOOD);

    fixture.server.set_bearer_token(FRESH);
    list_existing(&class, &mut backend).assert_is_an_authentication_failure();

    let installed = backend
        .0
        .inspect_connection(|sync| sync.client().set_credentials(Credentials::bearer(FRESH)));
    assert!(installed.is_some(), "there was no connection to refresh");

    let listed = list_existing(&class, &mut backend);
    assert!(
        listed.ok,
        "the connection did not pick up the fresh credentials"
    );
    assert_eq!(listed.count, 1);
}

#[test]
fn a_failure_that_is_not_a_401_is_still_reported_unchanged() {
    // Errors other than 401 (such as offline) are returned unchanged.
    let class = Class::get();
    let fixture = Fixture::start();
    let mut backend = fixture.detached(GOOD);
    assert!(backend.0.drop_connection());

    let listed = list_existing(&class, &mut backend);
    assert!(!listed.ok);
    assert!(!listed.error.is_null());
    // SAFETY: a live GError the struct owns and frees.
    unsafe {
        assert_eq!((*listed.error).domain, e_client_error_quark());
        assert_eq!(
            (*listed.error).code,
            eds_sys::E_CLIENT_ERROR_REPOSITORY_OFFLINE as i32
        );
    }
}
