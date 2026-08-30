// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `ECalMetaBackend` subclass: the type EDS registers, the vfunc slots it
//! dispatches through, and what those slots do with the connection the
//! instance holds.
//!
//! Every call here goes *through the class struct* rather than at the Rust
//! functions directly, because that is the only thing EDS ever does: a vfunc
//! that is correct but not installed is a backend that silently uses
//! `ECalMetaBackend`'s own defaults — an empty calendar and no error.
//!
//! What is not here is a real instance. Constructing one needs an
//! `ESourceRegistry` and so a running `evolution-source-registry` on the
//! session bus, which neither this VM nor CI has. [`JmapCalBackend::detached`]
//! is the stand-in: the parent bytes are zeroed and nothing may touch them,
//! which is exactly why `connect_sync` — the one vfunc that reads the parent's
//! `ESource` — is tested one layer down, in `tests/connect.rs`.

use std::ffi::{CStr, CString};
use std::mem::{MaybeUninit, size_of};
use std::ptr;
use std::thread;
use std::time::{Duration, Instant};

use eds_sys::{
    E_CAL_OPERATION_FLAG_NONE, E_CLIENT_ERROR_REPOSITORY_OFFLINE, ECalBackendSyncClass,
    ECalMetaBackend, ECalMetaBackendClass, ECalMetaBackendInfo, ICalComponent,
    e_cal_meta_backend_get_type, e_cal_meta_backend_info_free, e_client_error_quark,
};
use glib_sys::{
    GError, GFALSE, GSList, GTRUE, g_error_free, g_free, g_slist_free_full, g_slist_length,
    g_slist_nth_data, gchar,
};
use gobject_sys::{
    GTypeQuery, g_type_class_ref, g_type_class_unref, g_type_name, g_type_parent, g_type_query,
};
use jmap_backend_cal::backend::{JmapCalBackend, JmapCalBackendClass, parent_class};
use jmap_backend_cal::marshal;
use jmap_backend_core::push::PushRefresh;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use jmap_cal_sync::CalSync;
use jmap_client::eventsource::expand_url;
use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::CalendarEvent;

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

    /// The `ECalMetaBackendClass` half, which is where every slot we override
    /// lives.
    fn vfuncs(&self) -> &ECalMetaBackendClass {
        // SAFETY: the class is referenced and leads with the parent's.
        unsafe { &(*self.0).parent_class }
    }

    /// The `ECalBackendSyncClass` half, two levels up: `get_free_busy_sync` is
    /// declared there rather than on `ECalMetaBackend`, which merely fills it
    /// in like any other subclass would.
    fn sync_vfuncs(&self) -> &ECalBackendSyncClass {
        &self.vfuncs().parent_class
    }
}

impl Drop for Class {
    fn drop(&mut self) {
        // SAFETY: the reference taken in `get` is given back exactly once.
        unsafe { g_type_class_unref(self.0.cast()) };
    }
}

/// An instance the GObject type system knows nothing about — see the module
/// comment. Only the session slot may be touched through it.
struct Detached(Box<JmapCalBackend>);

impl Detached {
    fn new() -> Self {
        Self(JmapCalBackend::detached())
    }

    fn as_ptr(&mut self) -> *mut ECalMetaBackend {
        ptr::from_mut(&mut *self.0).cast()
    }
}

/// A mock server and the `CalSync` over its default calendar, so a detached
/// instance can be handed a connection without going through `connect_sync`.
struct Fixture {
    server: MockServer,
    account_id: Id,
    calendar: Id,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        let calendar = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            state
                .account_mut(&account_id)
                .unwrap()
                .seed_calendar("Personal", true)
        };
        Self {
            server,
            account_id,
            calendar,
        }
    }

    fn client(&self) -> Client {
        Client::connect(self.server.origin(), Credentials::none()).unwrap()
    }

    fn sync(&self) -> CalSync {
        CalSync::new(
            self.client(),
            self.account_id.clone(),
            self.calendar.clone(),
        )
    }

    fn seed(&self, title: &str, start: &str) -> Id {
        self.client()
            .event_create(
                &self.account_id,
                &CalendarEvent::simple(self.calendar.clone(), title, start, "PT1H"),
            )
            .unwrap()
            .id
            .expect("server assigned id")
    }
}

/// Asserts that a failed call reported the account as offline, and frees the
/// error. The code matters: it is what makes `ECalMetaBackend` serve its cache
/// rather than show the user a broken calendar.
unsafe fn assert_offline(error: &mut *mut GError) {
    unsafe {
        assert!(!error.is_null(), "the call failed without setting an error");
        assert_eq!((**error).domain, e_client_error_quark(), "error domain");
        assert_eq!(
            (**error).code,
            E_CLIENT_ERROR_REPOSITORY_OFFLINE as i32,
            "error code"
        );
        g_error_free(*error);
        *error = ptr::null_mut();
    }
}

// ---------------------------------------------------------------------------
// the type itself

#[test]
fn the_backend_registers_as_a_subclass_of_ecalmetabackend() {
    let gtype = register_static::<JmapCalBackend>();
    assert_ne!(gtype, 0);

    // SAFETY: `gtype` is a registered type.
    let name = unsafe { CStr::from_ptr(g_type_name(gtype)) };
    assert_eq!(name, JmapCalBackend::NAME);
    assert_eq!(
        unsafe { g_type_parent(gtype) },
        unsafe { e_cal_meta_backend_get_type() },
        "the backend must derive from ECalMetaBackend, or it gets none of the cache"
    );
}

/// The layout check `eds-sys` does for the EDS types, for ours: GObject
/// allocates from the sizes registration reported, so a struct that has grown
/// past what was registered is a heap overflow on every instance.
#[test]
fn the_registered_sizes_are_the_rust_struct_sizes() {
    let gtype = register_static::<JmapCalBackend>();
    let mut query = MaybeUninit::<GTypeQuery>::zeroed();
    // SAFETY: `gtype` is registered and `query` is writable.
    let query = unsafe {
        g_type_query(gtype, query.as_mut_ptr());
        query.assume_init()
    };

    assert_eq!(query.instance_size as usize, size_of::<JmapCalBackend>());
    assert_eq!(query.class_size as usize, size_of::<JmapCalBackendClass>());
    assert!(
        size_of::<JmapCalBackend>() > size_of::<ECalMetaBackend>(),
        "the instance struct must have room for the session on top of the parent"
    );
}

/// A vfunc that is implemented but not installed is the worst of both worlds:
/// EDS falls back to `ECalMetaBackend`'s own defaults, which report an empty
/// calendar rather than an error.
#[test]
fn class_init_installs_every_vfunc_the_meta_backend_dispatches_on() {
    let class = Class::get();
    let vfuncs = class.vfuncs();

    assert!(vfuncs.connect_sync.is_some(), "connect_sync");
    assert!(vfuncs.disconnect_sync.is_some(), "disconnect_sync");
    assert!(vfuncs.list_existing_sync.is_some(), "list_existing_sync");
    assert!(vfuncs.get_changes_sync.is_some(), "get_changes_sync");
    assert!(vfuncs.load_component_sync.is_some(), "load_component_sync");
    assert!(vfuncs.save_component_sync.is_some(), "save_component_sync");
    assert!(
        vfuncs.remove_component_sync.is_some(),
        "remove_component_sync"
    );
    assert!(vfuncs.source_changed.is_some(), "source_changed");
}

/// Unlike every other slot here, the base class never fills this one in — the
/// module comment traces why, from `e_cal_meta_backend_class_init`'s own
/// source: there is nothing to chain up to, so this override must not try.
#[test]
fn the_parent_leaves_source_changed_unfilled_so_there_is_nothing_to_chain_up_to() {
    let parent = parent_class().expect("ECalMetaBackendClass is initialised");
    assert!(
        parent.source_changed.is_none(),
        "ECalMetaBackend fills this slot itself"
    );
}

/// The two search slots stay the parent's. `ECalMetaBackend` answers a query by
/// running it over the offline cache, which is a complete answer for a calendar
/// it has just synced — and JMAP's `CalendarEvent/query` cannot express an
/// S-expression anyway. Overriding them with something partial would take the
/// working implementation away.
#[test]
fn the_search_slots_are_left_to_the_meta_backend_cache() {
    let class = Class::get();
    let parent = parent_class().expect("ECalMetaBackendClass is initialised");

    assert_eq!(
        class.vfuncs().search_sync.map(|f| f as usize),
        parent.search_sync.map(|f| f as usize),
        "search_sync was overridden"
    );
    assert_eq!(
        class.vfuncs().search_components_sync.map(|f| f as usize),
        parent.search_components_sync.map(|f| f as usize),
        "search_components_sync was overridden"
    );
}

/// `get_free_busy_sync` is not one of `ECalMetaBackend`'s own slots — it is
/// declared two classes up, on `ECalBackendSync`, which is where this backend
/// has to install it and where `E_CAL_BACKEND_SYNC_GET_CLASS` looks. Installed
/// on the wrong struct it would silently never be called, and the free/busy
/// panel would stay blank with nothing to show for it.
#[test]
fn class_init_installs_get_free_busy_on_the_sync_class_where_it_is_dispatched() {
    let class = Class::get();
    let parent = parent_class().expect("ECalMetaBackendClass is initialised");

    let ours = class
        .sync_vfuncs()
        .get_free_busy_sync
        .expect("get_free_busy_sync is not installed");
    assert_ne!(
        ours as usize,
        parent
            .parent_class
            .get_free_busy_sync
            .expect("ECalMetaBackend installs one") as usize,
        "the slot still holds ECalMetaBackend's implementation",
    );
}

/// And the parent's has to still be there to fall back on. It answers out of
/// the offline cache for the account's *own* address, which is the whole
/// answer when the account is offline and the only answer for the organiser
/// themselves on a server with no principals — so chaining up is not a
/// courtesy, it is where most of the useful data comes from.
#[test]
fn the_parent_class_offers_a_get_free_busy_sync_to_chain_up_to() {
    let _class = Class::get();
    let parent = parent_class().expect("ECalMetaBackendClass is initialised");
    assert!(
        parent.parent_class.get_free_busy_sync.is_some(),
        "ECalMetaBackend has no get_free_busy_sync of its own to fall back on"
    );
}

/// `get_changes_sync` answers "list the whole calendar instead" by chaining up
/// to the parent, which is the only implementation of a full diff against the
/// cache there is. If the parent has no such slot the fallback is a silent
/// no-op — a calendar that stays empty forever.
#[test]
fn the_parent_class_offers_a_get_changes_sync_to_chain_up_to() {
    let _class = Class::get();
    let parent = parent_class().expect("ECalMetaBackendClass is initialised");
    assert!(
        parent.get_changes_sync.is_some(),
        "ECalMetaBackend has no get_changes_sync of its own to fall back on"
    );
}

// ---------------------------------------------------------------------------
// dispatch without a connection

/// EDS calls `connect_sync` before anything else, so reaching an operation
/// without a connection means a disconnect raced it — which is what going
/// offline looks like. Reporting that as offline is what makes the meta
/// backend serve its cache; anything else is a visible failure for a state the
/// user asked for.
#[test]
fn every_operation_without_a_connection_reports_offline_instead_of_crashing() {
    let class = Class::get();
    let vfuncs = class.vfuncs();
    let mut backend = Detached::new();
    let uid = CString::new("whatever").unwrap();

    unsafe {
        let this = backend.as_ptr();
        let mut error: *mut GError = ptr::null_mut();

        let list = vfuncs.list_existing_sync.unwrap();
        assert_eq!(
            list(
                this,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut error
            ),
            GFALSE
        );
        assert_offline(&mut error);

        let load = vfuncs.load_component_sync.unwrap();
        let mut component: *mut ICalComponent = ptr::null_mut();
        assert_eq!(
            load(
                this,
                uid.as_ptr(),
                ptr::null(),
                &mut component,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut error,
            ),
            GFALSE
        );
        assert!(component.is_null());
        assert_offline(&mut error);

        let save = vfuncs.save_component_sync.unwrap();
        assert_eq!(
            save(
                this,
                GFALSE,
                0,
                ptr::null(),
                ptr::null(),
                E_CAL_OPERATION_FLAG_NONE,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut error,
            ),
            GFALSE
        );
        assert_offline(&mut error);

        let remove = vfuncs.remove_component_sync.unwrap();
        assert_eq!(
            remove(
                this,
                0,
                uid.as_ptr(),
                ptr::null(),
                ptr::null(),
                E_CAL_OPERATION_FLAG_NONE,
                ptr::null_mut(),
                &mut error,
            ),
            GFALSE
        );
        assert_offline(&mut error);

        let changes = vfuncs.get_changes_sync.unwrap();
        assert_eq!(
            changes(
                this,
                uid.as_ptr(),
                GFALSE,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut error,
            ),
            GFALSE
        );
        assert_offline(&mut error);
    }
}

// ---------------------------------------------------------------------------
// dispatch with one

#[test]
fn a_connected_backend_dispatches_list_existing_to_the_sync_layer() {
    let fixture = Fixture::start();
    let seeded = fixture.seed("Sprint review", "2026-03-04T09:00:00");

    let class = Class::get();
    let mut backend = Detached::new();
    backend.0.store_connection(fixture.sync());

    let mut tag: *mut gchar = ptr::null_mut();
    let mut objects: *mut GSList = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let list = class.vfuncs().list_existing_sync.unwrap();
        assert_eq!(
            list(
                backend.as_ptr(),
                &mut tag,
                &mut objects,
                ptr::null_mut(),
                &mut error,
            ),
            GTRUE
        );
        assert!(error.is_null());
        assert_eq!(g_slist_length(objects), 1);
        assert!(!tag.is_null(), "no sync tag came back");

        let node = g_slist_nth_data(objects, 0).cast::<ECalMetaBackendInfo>();
        assert!(!node.is_null());
        let uid = CStr::from_ptr((*node).uid).to_string_lossy().into_owned();
        assert_eq!(uid, seeded.to_string());

        g_free(tag.cast());
        g_slist_free_full(objects, Some(e_cal_meta_backend_info_free));
    }
}

#[test]
fn a_connected_backend_dispatches_load_component_to_the_sync_layer() {
    let fixture = Fixture::start();
    let id = fixture.seed("Sprint review", "2026-03-04T09:00:00");
    let uid = CString::new(id.to_string()).unwrap();

    let class = Class::get();
    let mut backend = Detached::new();
    backend.0.store_connection(fixture.sync());

    let mut component: *mut ICalComponent = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let load = class.vfuncs().load_component_sync.unwrap();
        assert_eq!(
            load(
                backend.as_ptr(),
                uid.as_ptr(),
                ptr::null(),
                &mut component,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut error,
            ),
            GTRUE
        );
        assert!(error.is_null());
        assert!(!component.is_null());
        let icalendar = marshal::ical_from_component(component).expect("rendered");
        assert!(icalendar.contains("SUMMARY:Sprint review"), "{icalendar}");
        marshal::component_unref(component);
    }
}

/// Going offline has to leave the backend in the state a fresh one is in, and
/// EDS is entitled to say it twice — it disconnects on shutdown as well as on
/// the offline transition.
#[test]
fn disconnect_drops_the_connection_and_saying_it_twice_is_still_success() {
    let fixture = Fixture::start();
    let class = Class::get();
    let mut backend = Detached::new();
    backend.0.store_connection(fixture.sync());
    assert!(backend.0.is_connected());

    unsafe {
        let disconnect = class.vfuncs().disconnect_sync.unwrap();
        let mut error: *mut GError = ptr::null_mut();

        assert_eq!(
            disconnect(backend.as_ptr(), ptr::null_mut(), &mut error),
            GTRUE
        );
        assert!(error.is_null());
        assert!(!backend.0.is_connected(), "the connection was kept");

        assert_eq!(
            disconnect(backend.as_ptr(), ptr::null_mut(), &mut error),
            GTRUE
        );
        assert!(error.is_null(), "a second disconnect is not a failure");

        // And the operations are back to reporting offline.
        let list = class.vfuncs().list_existing_sync.unwrap();
        assert_eq!(
            list(
                backend.as_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut error,
            ),
            GFALSE
        );
        assert_offline(&mut error);
    }
}

/// GObject frees the instance struct without running a Rust destructor over
/// it, so a connection the slot still holds at `finalize` is leaked — together
/// with its socket.
#[test]
fn finalize_drops_the_connection_the_instance_still_holds() {
    let fixture = Fixture::start();
    let mut backend = Detached::new();
    backend.0.store_connection(fixture.sync());

    // SAFETY: nothing else can reach this instance, which is what GObject
    // guarantees the real finalize.
    unsafe { JmapCalBackend::finalize(ptr::from_mut(&mut *backend.0)) };

    assert!(!backend.0.is_connected(), "finalize kept the connection");
}

// ---------------------------------------------------------------------------
// JMAP Push (docs/ROADMAP.md item 28)

/// A live push subscription against the fixture's own mock, which needs no
/// GObject — `PushRefresh` is deliberately given its refresh action rather
/// than deriving it from a backend, so a detached instance can hold one.
fn subscription(fixture: &Fixture) -> PushRefresh {
    let url = expand_url(
        &format!("{}/eventsource", fixture.server.origin()),
        &["CalendarEvent"],
        false,
        0,
    );
    PushRefresh::start(
        url,
        Vec::new(),
        fixture.account_id.clone(),
        vec!["CalendarEvent".to_owned()],
        |_types: &[String]| {},
    )
}

/// `disconnect_sync` has to end the push subscription as well as the
/// connection. A subscription that outlived its connection would keep
/// scheduling refreshes on a backend with nothing to refresh with — and, once
/// EDS dropped the backend, on nothing at all.
#[test]
fn disconnect_stops_the_push_subscription_with_the_connection() {
    let fixture = Fixture::start();
    let class = Class::get();
    let mut backend = Detached::new();
    backend.0.store_connection(fixture.sync());
    backend.0.store_push(subscription(&fixture));
    assert!(backend.0.is_pushing());

    unsafe {
        let disconnect = class.vfuncs().disconnect_sync.unwrap();
        let mut error: *mut GError = ptr::null_mut();
        assert_eq!(
            disconnect(backend.as_ptr(), ptr::null_mut(), &mut error),
            GTRUE
        );
        assert!(error.is_null());
    }

    assert!(
        !backend.0.is_pushing(),
        "disconnect left the push subscription running"
    );
    assert!(!backend.0.is_connected());
}

/// And `finalize` has to do it too, because EDS does not promise a
/// `disconnect_sync` first: `ecmb_dispose` cancels its own refreshes and
/// drops its own cache, but never asks the subclass to disconnect. A push
/// thread that survived here would be holding a `GWeakRef` to freed instance
/// memory.
#[test]
fn finalize_stops_a_push_subscription_no_disconnect_ever_reached() {
    let fixture = Fixture::start();
    let mut backend = Detached::new();
    backend.0.store_connection(fixture.sync());
    backend.0.store_push(subscription(&fixture));
    assert!(backend.0.is_pushing());

    // SAFETY: nothing else can reach this instance, which is what GObject
    // guarantees the real finalize.
    unsafe { JmapCalBackend::finalize(ptr::from_mut(&mut *backend.0)) };

    assert!(
        !backend.0.is_pushing(),
        "finalize kept the push subscription, and its thread"
    );
    assert!(!backend.0.is_connected());
}

/// The header-refresh piece of item 28: a push subscription's `Authorization`
/// header can go stale for the same reason the pooled connection's does — an
/// OAuth 2.0 access token rotates. `refresh_credentials` cannot be driven
/// end-to-end here since it needs a real `ESource`'s token exchange, but
/// `JmapCalBackend::refresh_push_headers` is the half it hands off to, and is
/// directly testable: a subscription started with a stale header is refused,
/// reconnects, and resumes listening once the fresh header replaces it.
#[test]
fn refresh_push_headers_lets_a_stalled_subscription_reconnect() {
    let server = MockServer::builder().bearer_token("fresh-token").start();
    let backend = Detached::new();
    let url = expand_url(
        &format!("{}/eventsource", server.origin()),
        &["CalendarEvent"],
        false,
        0,
    );
    backend.0.store_push(PushRefresh::start(
        url,
        vec![("Authorization".to_owned(), "Bearer stale-token".to_owned())],
        server.account_id(),
        vec!["CalendarEvent".to_owned()],
        |_types: &[String]| {},
    ));

    let deadline = Instant::now() + Duration::from_secs(5);
    while server.unauthorized_responses() == 0 {
        assert!(
            Instant::now() < deadline,
            "the stale token was never refused"
        );
        thread::sleep(Duration::from_millis(10));
    }

    backend.0.refresh_push_headers(vec![(
        "Authorization".to_owned(),
        "Bearer fresh-token".to_owned(),
    )]);

    server.wait_for_event_source_subscriber(Duration::from_secs(5));
}
