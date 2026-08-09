// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Stop button, and the vfuncs that had never looked at it.
//!
//! The address book's `tests/cancellation.rs` says the whole of it, and this
//! is the same story on `ECalMetaBackend`: every sync vfunc but `connect_sync`
//! named its `GCancellable` `_cancellable` and ignored it, so the only
//! cancellation that reached anything belonged to the account rather than to
//! the operation. `jmap-backend-core`'s `observe` is what the vfuncs now hold
//! for the length of their call.
//!
//! The calls go through the class struct, on a [`JmapCalBackend::detached`]
//! instance, and the cancellable is stopped before the call — see the address
//! book's module comment for why each of those is the right shape.

use std::ffi::CString;
use std::ptr;

use eds_sys::{
    E_CAL_OPERATION_FLAG_NONE, ECalComponent, ECalMetaBackend, ECalMetaBackendClass, ICalComponent,
    e_cal_component_new_from_string,
};
use gio_sys::{
    G_IO_ERROR_CANCELLED, GCancellable, g_cancellable_cancel, g_cancellable_new, g_io_error_quark,
};
use glib_sys::{
    GError, GFALSE, GSList, GTRUE, g_error_free, g_free, g_slist_free, g_slist_free_full,
    g_slist_prepend, gchar,
};
use gobject_sys::{g_object_unref, g_type_class_ref, g_type_class_unref};
use jmap_backend_cal::backend::{JmapCalBackend, JmapCalBackendClass};
use jmap_backend_core::subclass::register_static;
use jmap_cal_sync::CalSync;
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

/// An instance the GObject type system knows nothing about. Only the session
/// slot may be touched through it.
struct Detached(Box<JmapCalBackend>);

impl Detached {
    fn new(sync: CalSync) -> Self {
        let backend = JmapCalBackend::detached();
        backend.store_connection(sync);
        Self(backend)
    }

    fn as_ptr(&mut self) -> *mut ECalMetaBackend {
        ptr::from_mut(&mut *self.0).cast()
    }
}

/// A mock server with one event in its default calendar.
struct Fixture {
    server: MockServer,
    account_id: Id,
    calendar: Id,
    seeded: Id,
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
        let mut fixture = Self {
            server,
            account_id,
            calendar,
            seeded: Id::new("unseeded"),
        };
        fixture.seeded = fixture.seed("Standup", "2026-08-10T07:00:00Z");
        fixture
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

    /// What the server holds now, asked over a connection of its own — the
    /// vfunc's scope is gone by the time this runs, so this call is not
    /// cancelled by it.
    fn events_on_the_server(&self) -> usize {
        self.sync().list_existing().expect("listed").1.len()
    }
}

/// A `GCancellable` the user has already stopped.
struct Stopped(*mut GCancellable);

impl Stopped {
    fn new() -> Self {
        // SAFETY: constructing a GCancellable and cancelling it; both take
        // ownership of nothing.
        unsafe {
            let cancellable = g_cancellable_new();
            g_cancellable_cancel(cancellable);
            Self(cancellable)
        }
    }
}

impl Drop for Stopped {
    fn drop(&mut self) {
        // SAFETY: the one reference `g_cancellable_new` handed over.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// What a vfunc that refused set, freed the way EDS frees it.
struct Refusal(*mut GError);

impl Refusal {
    /// `G_IO_ERROR_CANCELLED`, which is what EDS tests for before it decides a
    /// calendar is broken and puts an alert in front of the user.
    fn is_the_stop_the_user_pressed(&self, what: &str) {
        assert!(!self.0.is_null(), "{what} did not report the cancellation");
        // SAFETY: a live GError this struct owns.
        unsafe {
            assert_eq!(
                (*self.0).domain,
                g_io_error_quark(),
                "{what} reported the cancellation in the wrong domain: {}",
                std::ffi::CStr::from_ptr((*self.0).message).to_string_lossy()
            );
            assert_eq!((*self.0).code, G_IO_ERROR_CANCELLED, "{what}");
        }
    }
}

impl Drop for Refusal {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the vfunc handed ownership of it over.
            unsafe { g_error_free(self.0) };
        }
    }
}

/// One instance of an event, as `save_component_sync` receives them.
fn instance(vevent: &str) -> *mut ECalComponent {
    let text = CString::new(vevent).expect("a component with no NUL");
    // SAFETY: the text is NUL-terminated and valid for the call.
    let component = unsafe { e_cal_component_new_from_string(text.as_ptr()) };
    assert!(!component.is_null(), "the instance did not parse: {vevent}");
    component
}

const NEW_EVENT: &str = "BEGIN:VEVENT\r\n\
                         UID:20260810T090000-1234@evolution\r\n\
                         SUMMARY:Never Saved\r\n\
                         DTSTART:20260810T070000Z\r\n\
                         DURATION:PT30M\r\n\
                         END:VEVENT\r\n";

/// The first sync of a calendar is the longest operation this backend has, and
/// the one a user is most likely to stop.
#[test]
fn a_listing_the_user_stopped_does_not_go_to_the_server() {
    let fixture = Fixture::start();
    let class = Class::get();
    let mut backend = Detached::new(fixture.sync());
    let stopped = Stopped::new();

    let mut tag: *mut gchar = ptr::null_mut();
    let mut objects: *mut GSList = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: an instance of the class, a live cancellable, and out-parameters
    // that are writable and currently NULL.
    let ok = unsafe {
        let list = class.vfuncs().list_existing_sync.expect("a listing vfunc");
        list(
            backend.as_ptr(),
            &mut tag,
            &mut objects,
            stopped.0,
            &mut error,
        )
    };

    assert_eq!(ok, GFALSE, "the listing claimed to have happened");
    assert!(tag.is_null(), "a cancelled listing produced a sync tag");
    assert!(objects.is_null(), "a cancelled listing produced components");
    Refusal(error).is_the_stop_the_user_pressed("list_existing_sync");
}

/// The incremental sync, which runs on every refresh.
#[test]
fn a_changes_call_the_user_stopped_does_not_go_to_the_server() {
    let fixture = Fixture::start();
    let class = Class::get();
    let mut backend = Detached::new(fixture.sync());
    let stopped = Stopped::new();

    // A tag, so the vfunc asks the server for a diff rather than answering
    // "list the whole calendar instead" without a request.
    let tag = CString::new("0").expect("a tag with no NUL");
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: as above; `tag` is NUL-terminated and alive across the call.
    let ok = unsafe {
        let changes = class.vfuncs().get_changes_sync.expect("a changes vfunc");
        changes(
            backend.as_ptr(),
            tag.as_ptr(),
            GFALSE,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            stopped.0,
            &mut error,
        )
    };

    assert_eq!(ok, GFALSE, "the changes call claimed to have happened");
    Refusal(error).is_the_stop_the_user_pressed("get_changes_sync");
}

#[test]
fn a_component_the_user_stopped_loading_is_not_fetched() {
    let fixture = Fixture::start();
    let class = Class::get();
    let mut backend = Detached::new(fixture.sync());
    let stopped = Stopped::new();

    let uid = CString::new(fixture.seeded.to_string()).expect("a uid with no NUL");
    let mut component: *mut ICalComponent = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: as above; `uid` is NUL-terminated and alive across the call.
    let ok = unsafe {
        let load = class.vfuncs().load_component_sync.expect("a load vfunc");
        load(
            backend.as_ptr(),
            uid.as_ptr(),
            ptr::null(),
            &mut component,
            ptr::null_mut(),
            stopped.0,
            &mut error,
        )
    };

    assert_eq!(ok, GFALSE, "the load claimed to have happened");
    assert!(component.is_null(), "a cancelled load produced a component");
    Refusal(error).is_the_stop_the_user_pressed("load_component_sync");
}

/// A write, not a read: what the user stopped here is a change to their
/// calendar, and it must not be on the server afterwards.
#[test]
fn a_component_the_user_stopped_saving_is_not_written() {
    let fixture = Fixture::start();
    let before = fixture.events_on_the_server();

    let class = Class::get();
    let mut backend = Detached::new(fixture.sync());
    let stopped = Stopped::new();

    let component = instance(NEW_EVENT);
    // SAFETY: `component` outlives the list and the call below.
    let list = unsafe { g_slist_prepend(ptr::null_mut(), component.cast()) };

    let mut new_uid: *mut gchar = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: as above; `list` is the instance list EDS's contract describes.
    let ok = unsafe {
        let save = class.vfuncs().save_component_sync.expect("a save vfunc");
        save(
            backend.as_ptr(),
            GFALSE,
            0,
            list,
            ptr::null(),
            E_CAL_OPERATION_FLAG_NONE,
            &mut new_uid,
            ptr::null_mut(),
            stopped.0,
            &mut error,
        )
    };

    // SAFETY: the list and the reference the test owns, freed the way EDS
    // frees them.
    unsafe {
        g_slist_free(list);
        g_object_unref(component.cast());
    }

    assert_eq!(ok, GFALSE, "the save claimed to have happened");
    assert!(new_uid.is_null(), "a cancelled save produced a uid");
    Refusal(error).is_the_stop_the_user_pressed("save_component_sync");
    assert_eq!(
        fixture.events_on_the_server(),
        before,
        "the cancelled save reached the server anyway"
    );
}

/// The other write, and the one where getting it wrong loses data.
#[test]
fn a_component_the_user_stopped_removing_is_still_there() {
    let fixture = Fixture::start();
    let before = fixture.events_on_the_server();

    let class = Class::get();
    let mut backend = Detached::new(fixture.sync());
    let stopped = Stopped::new();

    let uid = CString::new(fixture.seeded.to_string()).expect("a uid with no NUL");
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: as above; `uid` is NUL-terminated and alive across the call.
    let ok = unsafe {
        let remove = class
            .vfuncs()
            .remove_component_sync
            .expect("a remove vfunc");
        remove(
            backend.as_ptr(),
            0,
            uid.as_ptr(),
            ptr::null(),
            ptr::null(),
            E_CAL_OPERATION_FLAG_NONE,
            stopped.0,
            &mut error,
        )
    };

    assert_eq!(ok, GFALSE, "the removal claimed to have happened");
    Refusal(error).is_the_stop_the_user_pressed("remove_component_sync");
    assert_eq!(
        fixture.events_on_the_server(),
        before,
        "the cancelled removal deleted the event anyway"
    );
}

/// A NULL cancellable is GIO's "this call cannot be cancelled", and must not
/// be read as "already stopped".
#[test]
fn an_operation_with_no_cancellable_at_all_still_runs() {
    let fixture = Fixture::start();
    let class = Class::get();
    let mut backend = Detached::new(fixture.sync());

    let mut tag: *mut gchar = ptr::null_mut();
    let mut objects: *mut GSList = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: as above, with the NULL cancellable EDS is entitled to pass.
    let ok = unsafe {
        let list = class.vfuncs().list_existing_sync.expect("a listing vfunc");
        list(
            backend.as_ptr(),
            &mut tag,
            &mut objects,
            ptr::null_mut(),
            &mut error,
        )
    };

    assert_eq!(ok, GTRUE, "the listing failed with no cancellable");
    assert!(error.is_null());
    // SAFETY: both were allocated by GLib and ownership passed to this test.
    unsafe {
        g_free(tag.cast());
        g_slist_free_full(objects, Some(eds_sys::e_cal_meta_backend_info_free));
    }
}
