// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A pooled connection whose bearer token went stale.
//!
//! `with_connection` now runs every vfunc body against a private `GError`
//! slot so that a 401 can be answered with a refreshed token and one retry
//! instead of a consent window. The refresh itself needs an `ESource` with an
//! `EOAuth2Service` behind it, which needs `evolution-source-registry` on the
//! session bus, which neither this VM nor CI has — so the decision and the
//! `GError` bookkeeping are tested where they live, as
//! `jmap_backend_core::retry::retry_on_authentication_failure`'s own unit
//! tests, with plain closures.
//!
//! What is tested *here* is the half of it that needs a real vfunc dispatch:
//! that a genuinely stale token still reaches EDS as
//! `E_CLIENT_ERROR_AUTHENTICATION_FAILED` when there is nothing to refresh
//! from, that it does so having tried exactly once (a backend that retried a
//! doomed request would double every 401 the user's password already caused),
//! and that installing fresh credentials on the live connection makes the very
//! next vfunc call succeed — which is what the refresh does, through a read
//! guard the retry holds across both attempts.
//!
//! The server genuinely rotates the token it accepts
//! (`MockServer::set_bearer_token`); the client is not sabotaged into sending
//! something the server never took.

use std::ptr;

use eds_sys::{
    ECalMetaBackend, ECalMetaBackendClass, e_cal_meta_backend_info_free, e_client_error_quark,
};
use glib_sys::{GError, GSList, GTRUE, g_error_free, g_free, g_slist_free_full, gchar};
use gobject_sys::{g_type_class_ref, g_type_class_unref};
use jmap_backend_cal::backend::{JmapCalBackend, JmapCalBackendClass};
use jmap_backend_core::subclass::register_static;
use jmap_cal_sync::CalSync;
use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::CalendarEvent;

const GOOD: &str = "token-the-connection-was-built-with";
const FRESH: &str = "token-a-refresh-would-hand-back";

/// The class EDS would dispatch through, kept referenced for the test's
/// duration so the vfunc pointers stay valid.
struct Class(*mut JmapCalBackendClass);

impl Class {
    fn get() -> Self {
        let gtype = register_static::<JmapCalBackend>();
        assert_ne!(gtype, 0, "the backend type did not register");
        // SAFETY: the type is registered, so referencing its class runs
        // class_init and hands back a class struct of our own layout.
        Self(unsafe { g_type_class_ref(gtype) }.cast())
    }

    fn vfuncs(&self) -> &ECalMetaBackendClass {
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

/// An instance the GObject type system knows nothing about, holding one
/// connection. Only the session slot may be touched through it — which is why
/// `e_backend_get_source` finds nothing and no refresh is attempted, exactly
/// the "not an OAuth 2.0 account" path a Basic-password account takes.
struct Detached(Box<JmapCalBackend>);

impl Detached {
    fn as_ptr(&mut self) -> *mut ECalMetaBackend {
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

    // SAFETY: a valid instance of ours, a NULL cancellable, and three
    // writable, currently-NULL out-parameters — what EDS passes.
    let ok = unsafe {
        vfunc(
            backend.as_ptr(),
            &mut tag,
            &mut objects,
            ptr::null_mut(),
            &mut error,
        )
    };

    // SAFETY: the out-parameters are transfer-full when the call succeeded and
    // NULL otherwise, and the list is of `ECalMetaBackendInfo` — freed the way
    // every other test in this crate frees one.
    let count = unsafe {
        let count = glib_sys::g_slist_length(objects) as usize;
        g_slist_free_full(objects, Some(e_cal_meta_backend_info_free));
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

/// A mock that accepts exactly one bearer token at a time, with one event in
/// its default calendar.
struct Fixture {
    server: MockServer,
    account_id: Id,
    calendar: Id,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().bearer_token(GOOD).start();
        let account_id = server.account_id();
        let calendar = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            state
                .account_mut(&account_id)
                .unwrap()
                .seed_calendar("Personal", true)
        };
        let fixture = Self {
            server,
            account_id,
            calendar,
        };
        fixture
            .client(GOOD)
            .event_create(
                &fixture.account_id,
                &CalendarEvent::simple(
                    fixture.calendar.clone(),
                    "Standup",
                    "2026-08-10T07:00:00Z",
                    "PT1H",
                ),
            )
            .expect("seeded an event");
        fixture
    }

    fn client(&self, token: &str) -> Client {
        Client::connect(self.server.origin(), Credentials::bearer(token)).expect("connected")
    }

    fn detached(&self, token: &str) -> Detached {
        let backend = JmapCalBackend::detached();
        backend.store_connection(CalSync::new(
            self.client(token),
            self.account_id.clone(),
            self.calendar.clone(),
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
    // carries the old one, which is the hourly bug exactly.
    fixture.server.set_bearer_token(FRESH);
    let before = fixture.server.unauthorized_responses();
    let listed = list_existing(&class, &mut backend);
    listed.assert_is_an_authentication_failure();

    // A detached instance has no `ESource`, so `refresh_credentials` reports
    // "nothing to refresh" and the retry never runs — the same path a
    // Basic-password account takes, where a re-fetch would only reproduce the
    // wrong secret. One refused request, not two.
    assert_eq!(
        fixture.server.unauthorized_responses() - before,
        1,
        "a 401 with no refreshable credentials must not be retried"
    );
}

#[test]
fn fresh_credentials_installed_on_the_live_connection_fix_the_very_next_vfunc_call() {
    // What the refresh does, minus the `ESource` this test cannot have: the
    // retry replaces the credentials *on* the pooled connection rather than
    // replacing the connection, so it can hold a read guard across both
    // attempts and never block the other threads' operations. This pins that
    // a vfunc dispatched afterwards sees the new token.
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
    // The private `GError` slot the retry introduced must hand the caller
    // exactly the error the attempt produced, not a rewritten one: a
    // disconnected backend is still `REPOSITORY_OFFLINE`, which is what makes
    // `ECalMetaBackend` serve its cache instead of showing an alert.
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
