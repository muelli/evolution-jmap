// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `ECalMetaBackend` vfunc bodies, called the way EDS calls them:
//! out-parameters that start NULL, a `GError **` that starts NULL, and a return
//! value that says which of the two was written.
//!
//! Every test runs against `jmap-mockd`, so the assertions are about what the
//! server was actually told. What is deliberately *not* here is a live
//! `ECalMetaBackend`: constructing one needs an `ESourceRegistry` and so a
//! running `evolution-source-registry` on the session bus. Keeping the vfunc
//! bodies in a layer that takes a `&CalSync` is what lets them be tested at all.

use std::ffi::{CStr, CString};
use std::ptr;

use eds_sys::{
    E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND, E_CLIENT_ERROR_INVALID_ARG,
    E_CLIENT_ERROR_REPOSITORY_OFFLINE, ECalComponent, ECalMetaBackendInfo, ICalComponent,
    e_cal_client_error_quark, e_cal_component_get_icalcomponent, e_cal_component_new_from_string,
    e_cal_meta_backend_info_free, e_client_error_quark, i_cal_component_set_uid,
};
use glib_sys::{
    GError, GFALSE, GSList, GTRUE, g_error_free, g_free, g_slist_free, g_slist_free_full,
    g_slist_length, g_slist_nth_data, g_slist_prepend, gboolean, gchar,
};
use gobject_sys::g_object_unref;
use jmap_backend_cal::marshal;
use jmap_backend_cal::ops::{self, Outcome};
use jmap_cal_sync::{CalSync, SyncError, Unsendable};
use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::CalendarEvent;

/// A mock server with two calendars, so "only this calendar" stays observable,
/// and the `CalSync` over the one the backend syncs.
struct Fixture {
    server: MockServer,
    account_id: Id,
    ours: Id,
    theirs: Id,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        let (ours, theirs) = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            (
                account.seed_calendar("Personal", true),
                account.seed_calendar("Team", false),
            )
        };
        Self {
            server,
            account_id,
            ours,
            theirs,
        }
    }

    fn client(&self) -> Client {
        Client::connect(self.server.origin(), Credentials::none()).unwrap()
    }

    fn sync(&self) -> CalSync {
        CalSync::new(self.client(), self.account_id.clone(), self.ours.clone())
    }

    /// Create an event directly, bypassing the code under test.
    fn seed(&self, calendar: &Id, title: &str, start: &str) -> Id {
        self.client()
            .event_create(
                &self.account_id,
                &CalendarEvent::simple(calendar.clone(), title, start, "PT1H"),
            )
            .unwrap()
            .id
            .expect("server assigned id")
    }

    /// The uids the calendar holds, as the server sees them.
    fn uids(&self) -> Vec<String> {
        let (_, events) = self.sync().list_existing().unwrap();
        let mut uids: Vec<String> = events.into_iter().map(|info| info.uid).collect();
        uids.sort();
        uids
    }
}

/// The four out-parameters EDS hands `get_changes_sync`, plus the sync tag.
struct ChangeOuts {
    tag: *mut gchar,
    repeat: gboolean,
    created: *mut GSList,
    modified: *mut GSList,
    removed: *mut GSList,
}

impl Default for ChangeOuts {
    /// `repeat` starts TRUE, which is *not* what EDS does — it passes a FALSE
    /// it initialised itself. Starting from the other value is what makes "the
    /// answer is always no" an assertion rather than a coincidence: a body that
    /// never writes the parameter would otherwise look identical to one that
    /// answers correctly.
    fn default() -> Self {
        Self {
            tag: ptr::null_mut(),
            repeat: GTRUE,
            created: ptr::null_mut(),
            modified: ptr::null_mut(),
            removed: ptr::null_mut(),
        }
    }
}

impl Drop for ChangeOuts {
    /// All three lists are freed as `ECalMetaBackendInfo`s — including the
    /// removals, which is where the calendar differs from the address book.
    fn drop(&mut self) {
        unsafe {
            g_free(self.tag.cast());
            g_slist_free_full(self.created, Some(e_cal_meta_backend_info_free));
            g_slist_free_full(self.modified, Some(e_cal_meta_backend_info_free));
            g_slist_free_full(self.removed, Some(e_cal_meta_backend_info_free));
        }
    }
}

/// Reads a `GSList` node as an `ECalMetaBackendInfo`, the way
/// `e_cal_meta_backend_process_changes_sync` does.
unsafe fn nth_info(list: *mut GSList, n: u32) -> (String, Option<String>, Option<String>) {
    unsafe {
        let node = g_slist_nth_data(list, n).cast::<ECalMetaBackendInfo>();
        assert!(!node.is_null(), "no node {n}");
        let text = |p: *mut gchar| {
            (!p.is_null()).then(|| CStr::from_ptr(p).to_string_lossy().into_owned())
        };
        (
            text((*node).uid).expect("an info without a uid identifies nothing"),
            text((*node).revision),
            text((*node).object),
        )
    }
}

unsafe fn take_string(out: &mut *mut gchar) -> String {
    unsafe {
        assert!(!out.is_null(), "the out-parameter was left NULL");
        let text = CStr::from_ptr(*out).to_string_lossy().into_owned();
        g_free(out.cast());
        *out = ptr::null_mut();
        text
    }
}

/// Asserts that a failed call set an error of exactly this domain and code, and
/// frees it. Getting it wrong is not cosmetic: Evolution branches on the pair,
/// and `ECalMetaBackend` itself branches on `OBJECT_NOT_FOUND`.
unsafe fn assert_error(error: &mut *mut GError, domain: u32, code: i32) {
    unsafe {
        assert!(!error.is_null(), "the call failed without setting an error");
        assert_eq!((**error).domain, domain, "error domain");
        assert_eq!((**error).code, code, "error code");
        assert!(!(**error).message.is_null(), "the error has no message");
        g_error_free(*error);
        *error = ptr::null_mut();
    }
}

/// One instance of an event, as `save_component_sync` receives them.
fn instance(vevent: &str) -> *mut ECalComponent {
    let text = CString::new(vevent).unwrap();
    // SAFETY: the text is NUL-terminated and valid for the call.
    let component = unsafe { e_cal_component_new_from_string(text.as_ptr()) };
    assert!(!component.is_null(), "the instance did not parse: {vevent}");
    component
}

/// The `GSList` of instances EDS passes. The components stay owned by the
/// caller, which is the ownership the vfunc has.
fn instance_list(components: &[*mut ECalComponent]) -> *mut GSList {
    let mut list = ptr::null_mut();
    for component in components.iter().rev() {
        // SAFETY: `list` is a valid GSList and the payload outlives it.
        list = unsafe { g_slist_prepend(list, component.cast()) };
    }
    list
}

/// Frees the list and the instances in it, which is EDS's half of the contract.
unsafe fn drop_instances(list: *mut GSList, components: &[*mut ECalComponent]) {
    unsafe {
        g_slist_free(list);
        for component in components {
            g_object_unref(component.cast());
        }
    }
}

// ---------------------------------------------------------------------------
// list_existing_sync

#[test]
fn list_existing_hands_back_one_node_per_event_in_this_calendar() {
    let fixture = Fixture::start();
    let mine = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    fixture.seed(&fixture.theirs, "Their offsite", "2026-01-15T10:00:00");

    let mut tag: *mut gchar = ptr::null_mut();
    let mut objects: *mut GSList = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let ok = ops::list_existing(&fixture.sync(), &mut tag, &mut objects, &mut error);

        assert_eq!(ok, GTRUE);
        assert!(error.is_null(), "a successful call must not set an error");
        assert_eq!(g_slist_length(objects), 1, "the other calendar leaked in");

        let (uid, revision, object) = nth_info(objects, 0);
        assert_eq!(uid, mine.to_string());
        assert!(revision.is_some_and(|r| !r.is_empty()), "no change token");
        let object = object.expect("a listed event carries its object");
        assert!(object.contains("SUMMARY:Standup"), "{object}");

        assert!(!take_string(&mut tag).is_empty(), "no sync tag");
        g_slist_free_full(objects, Some(e_cal_meta_backend_info_free));
    }
}

/// EDS reads "no objects" as a NULL list; the sync tag is still needed, or the
/// next sync has no state to go from.
#[test]
fn an_empty_calendar_lists_as_a_null_list_with_a_sync_tag() {
    let fixture = Fixture::start();
    fixture.seed(&fixture.theirs, "Their offsite", "2026-01-15T10:00:00");

    let mut tag: *mut gchar = ptr::null_mut();
    let mut objects: *mut GSList = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        assert_eq!(
            ops::list_existing(&fixture.sync(), &mut tag, &mut objects, &mut error),
            GTRUE
        );
        assert!(objects.is_null());
        assert!(!take_string(&mut tag).is_empty());
    }
}

/// A NULL out-parameter is GLib's "the caller does not want this one". It has
/// to be skipped rather than written through, and the list it would have held
/// must not be built at all — there would be nobody to free it.
#[test]
fn out_parameters_the_caller_did_not_ask_for_are_skipped() {
    let fixture = Fixture::start();
    fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let ok = ops::list_existing(
            &fixture.sync(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut error,
        );
        assert_eq!(ok, GTRUE);
        assert!(error.is_null());
    }
}

// ---------------------------------------------------------------------------
// load_component_sync

#[test]
fn load_component_yields_an_icalcomponent_keyed_by_the_jmap_id() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let uid = CString::new(id.to_string()).unwrap();

    let mut component: *mut ICalComponent = ptr::null_mut();
    let mut extra: *mut gchar = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let ok = ops::load_component(
            &fixture.sync(),
            uid.as_ptr(),
            &mut component,
            &mut extra,
            &mut error,
        );

        assert_eq!(ok, GTRUE);
        assert!(error.is_null());
        assert!(!component.is_null(), "no component was written");
        assert_eq!(
            marshal::component_uid(component).as_deref(),
            Some(id.as_str())
        );
        let text = marshal::ical_from_component(component).expect("rendered");
        assert!(text.contains("SUMMARY:Standup"), "{text}");
        marshal::component_unref(component);
    }
}

/// `ECalMetaBackend` matches on this exact domain and code to decide that a
/// component is gone rather than that the sync failed, so a not-found reported
/// any other way is a cache entry that never goes away.
#[test]
fn loading_an_unknown_component_reports_object_not_found_and_writes_nothing() {
    let fixture = Fixture::start();
    let uid = CString::new("no-such-event").unwrap();

    let mut component: *mut ICalComponent = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let ok = ops::load_component(
            &fixture.sync(),
            uid.as_ptr(),
            &mut component,
            ptr::null_mut(),
            &mut error,
        );

        assert_eq!(ok, GFALSE);
        assert!(component.is_null(), "a failed load must leave the out NULL");
        assert_error(
            &mut error,
            e_cal_client_error_quark(),
            E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND as i32,
        );
    }
}

#[test]
fn loading_without_an_identifier_is_an_invalid_argument_not_a_null_dereference() {
    let fixture = Fixture::start();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let ok = ops::load_component(
            &fixture.sync(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut error,
        );
        assert_eq!(ok, GFALSE);
        assert_error(
            &mut error,
            e_client_error_quark(),
            E_CLIENT_ERROR_INVALID_ARG as i32,
        );
    }
}

// ---------------------------------------------------------------------------
// save_component_sync

/// What Evolution hands a backend for a brand-new appointment: instances
/// carrying a `UID` it invented locally, which is not a JMAP id and must not
/// become one.
const NEW_EVENT: &str = "BEGIN:VEVENT\r\n\
                         UID:20260810T090000-1234@evolution\r\n\
                         SUMMARY:Standup\r\n\
                         DTSTART:20260810T070000Z\r\n\
                         DURATION:PT30M\r\n\
                         END:VEVENT\r\n";

#[test]
fn saving_a_new_component_creates_it_under_the_identifier_the_server_assigns() {
    let fixture = Fixture::start();
    let components = [instance(NEW_EVENT)];
    let list = instance_list(&components);

    let mut new_uid: *mut gchar = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let ok = ops::save_component(
            &fixture.sync(),
            GFALSE,
            list,
            ptr::null_mut(),
            &mut new_uid,
            ptr::null_mut(),
            &mut error,
        );

        assert_eq!(ok, GTRUE);
        assert!(error.is_null());
        let assigned = take_string(&mut new_uid);
        assert_ne!(
            assigned, "20260810T090000-1234@evolution",
            "the local uid reached the server"
        );
        assert_eq!(fixture.uids(), vec![assigned]);
        drop_instances(list, &components);
    }
}

#[test]
fn saving_an_existing_component_patches_it_rather_than_adding_a_second() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let sync = fixture.sync();
    let stored = sync.load_component(id.as_str()).unwrap().icalendar;
    // The master as EDS holds it: the cached object, edited, handed back as an
    // instance rather than as the envelope it was cached in.
    let edited = stored
        .replace("SUMMARY:Standup", "SUMMARY:Standup (short)")
        .replace("BEGIN:VCALENDAR\r\n", "")
        .replace("END:VCALENDAR\r\n", "");
    let vevent = edited
        .split_once("BEGIN:VEVENT")
        .map(|(_, rest)| format!("BEGIN:VEVENT{rest}"))
        .expect("the cached object holds an event");
    let components = [instance(&vevent)];
    let list = instance_list(&components);

    let mut new_uid: *mut gchar = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let ok = ops::save_component(
            &sync,
            GTRUE,
            list,
            ptr::null_mut(),
            &mut new_uid,
            ptr::null_mut(),
            &mut error,
        );

        assert_eq!(ok, GTRUE);
        assert!(error.is_null());
        assert_eq!(take_string(&mut new_uid), id.to_string());
        assert_eq!(fixture.uids(), vec![id.to_string()], "a duplicate was made");
        assert!(
            sync.load_component(id.as_str())
                .unwrap()
                .icalendar
                .contains("SUMMARY:Standup (short)")
        );
        drop_instances(list, &components);
    }
}

/// An edit whose master carries no identifier would otherwise be sent as a
/// create, which silently duplicates the user's appointment on the server. A
/// visible failure is the better answer.
///
/// The uid has to be *emptied* to get here, and that is the point: an
/// `ECalComponent` built from text with no `UID` invents one — `e_util_generate_uid`
/// — so a component that reached EDS intact always has an identifier of some
/// kind. Which is exactly why this guard cannot be dropped as unreachable: what
/// it defends against is a uid that reads back as nothing, and EDS's own
/// generosity is what would otherwise hide that.
#[test]
fn an_edit_without_an_identifier_is_refused_rather_than_duplicating() {
    let fixture = Fixture::start();
    let components = [instance(
        "BEGIN:VEVENT\r\nSUMMARY:Nameless\r\nDTSTART:20260810T070000Z\r\nEND:VEVENT\r\n",
    )];
    // SAFETY: the component is live and lends out the one it carries.
    unsafe {
        let inner = e_cal_component_get_icalcomponent(components[0]);
        i_cal_component_set_uid(inner, c"".as_ptr());
    }
    let list = instance_list(&components);

    let mut new_uid: *mut gchar = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let ok = ops::save_component(
            &fixture.sync(),
            GTRUE,
            list,
            ptr::null_mut(),
            &mut new_uid,
            ptr::null_mut(),
            &mut error,
        );

        assert_eq!(ok, GFALSE);
        assert!(new_uid.is_null());
        assert_error(
            &mut error,
            e_client_error_quark(),
            E_CLIENT_ERROR_INVALID_ARG as i32,
        );
        assert!(
            fixture.uids().is_empty(),
            "the calendar was written to anyway"
        );
        drop_instances(list, &components);
    }
}

/// No master among the instances — only a detached occurrence, or no instances
/// at all. There is nothing honest to send, and the marshalling says so; the
/// vfunc has to turn that into a failure rather than into a silent no-op.
#[test]
fn saving_instances_with_no_master_is_an_invalid_argument() {
    let fixture = Fixture::start();
    let components = [instance(
        "BEGIN:VEVENT\r\nUID:K1\r\nRECURRENCE-ID:20260812T070000Z\r\n\
         SUMMARY:Standup, moved\r\nDTSTART:20260812T080000Z\r\nEND:VEVENT\r\n",
    )];
    let list = instance_list(&components);
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let ok = ops::save_component(
            &fixture.sync(),
            GFALSE,
            list,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut error,
        );
        assert_eq!(ok, GFALSE);
        assert_error(
            &mut error,
            e_client_error_quark(),
            E_CLIENT_ERROR_INVALID_ARG as i32,
        );
        assert!(fixture.uids().is_empty());
        drop_instances(list, &components);
    }

    let mut error: *mut GError = ptr::null_mut();
    unsafe {
        let ok = ops::save_component(
            &fixture.sync(),
            GFALSE,
            ptr::null(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut error,
        );
        assert_eq!(ok, GFALSE);
        assert_error(
            &mut error,
            e_client_error_quark(),
            E_CLIENT_ERROR_INVALID_ARG as i32,
        );
    }
}

// ---------------------------------------------------------------------------
// remove_component_sync

#[test]
fn removing_a_component_destroys_it_on_the_server() {
    let fixture = Fixture::start();
    let doomed = fixture.seed(&fixture.ours, "Cancelled offsite", "2026-01-16T09:00:00");
    let kept = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let uid = CString::new(doomed.to_string()).unwrap();

    let mut error: *mut GError = ptr::null_mut();
    unsafe {
        assert_eq!(
            ops::remove_component(&fixture.sync(), uid.as_ptr(), &mut error),
            GTRUE
        );
        assert!(error.is_null());
    }
    assert_eq!(fixture.uids(), vec![kept.to_string()]);
}

#[test]
fn removing_nothing_is_an_invalid_argument_not_a_null_dereference() {
    let fixture = Fixture::start();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let ok = ops::remove_component(&fixture.sync(), ptr::null(), &mut error);
        assert_eq!(ok, GFALSE);
        assert_error(
            &mut error,
            e_client_error_quark(),
            E_CLIENT_ERROR_INVALID_ARG as i32,
        );
    }
}

// ---------------------------------------------------------------------------
// get_changes_sync

#[test]
fn get_changes_reports_changed_events_and_the_ones_that_are_gone() {
    let fixture = Fixture::start();
    let doomed = fixture.seed(&fixture.ours, "Cancelled offsite", "2026-01-16T09:00:00");
    let sync = fixture.sync();
    let (state, _) = sync.list_existing().unwrap();
    let tag = CString::new(state.as_str()).unwrap();

    let created = fixture.seed(&fixture.ours, "Retro", "2026-01-17T15:00:00");
    sync.remove_component(doomed.as_str()).unwrap();

    let mut outs = ChangeOuts::default();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let outcome = ops::get_changes(
            &sync,
            tag.as_ptr(),
            GFALSE,
            &mut outs.tag,
            &mut outs.repeat,
            &mut outs.created,
            &mut outs.modified,
            &mut outs.removed,
            &mut error,
        );

        assert!(matches!(outcome, Outcome::Reported), "{outcome:?}");
        assert!(error.is_null());
        assert_eq!(outs.repeat, GFALSE, "the paging is done inside get_changes");
        assert!(!outs.tag.is_null(), "no sync tag for the next round");

        assert_eq!(g_slist_length(outs.modified), 1);
        assert_eq!(nth_info(outs.modified, 0).0, created.to_string());
        assert_eq!(g_slist_length(outs.removed), 1);
        // A removal is an info carrying only its uid, not a bare string.
        assert_eq!(
            nth_info(outs.removed, 0),
            (doomed.to_string(), None, None),
            "a removal must not claim a revision or an object"
        );
    }
}

/// EDS sets `is_repeat` when it is coming back for the rest of a delta. This
/// backend never asks it to — the paging happens inside `CalSync::get_changes`
/// and `out_repeat` is always FALSE — so the flag can only arrive as a caller's
/// own bookkeeping, and the delta a tag names is the same either way.
#[test]
fn a_repeat_call_answers_from_the_tag_just_like_the_first_one() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let (state, _) = sync.list_existing().unwrap();
    let tag = CString::new(state.as_str()).unwrap();
    let created = fixture.seed(&fixture.ours, "Retro", "2026-01-17T15:00:00");

    for is_repeat in [GFALSE, GTRUE] {
        let mut outs = ChangeOuts::default();
        let mut error: *mut GError = ptr::null_mut();

        unsafe {
            let outcome = ops::get_changes(
                &sync,
                tag.as_ptr(),
                is_repeat,
                &mut outs.tag,
                &mut outs.repeat,
                &mut outs.created,
                &mut outs.modified,
                &mut outs.removed,
                &mut error,
            );

            assert!(matches!(outcome, Outcome::Reported), "{outcome:?}");
            assert_eq!(outs.repeat, GFALSE);
            assert_eq!(g_slist_length(outs.modified), 1);
            assert_eq!(nth_info(outs.modified, 0).0, created.to_string());
        }
    }
}

/// The first sync has no tag to go from. Answering it with an empty delta would
/// leave the calendar permanently empty, so the meta backend's own
/// implementation — list the calendar and diff it against the cache — has to
/// run.
///
/// The server is stopped first, which is what makes this an assertion rather
/// than a coincidence: an absent tag sent on as an empty `sinceState` would
/// come back a transport failure.
///
/// Both spellings of "absent" are checked. The EDS cache writes NULL, but an
/// empty string reaches the same place through a hand-edited cache — and `""`
/// handed back as a `sinceState` is a state, not the absence of one.
#[test]
fn get_changes_without_a_sync_tag_asks_for_a_full_listing_without_asking_the_server() {
    let fixture = Fixture::start();
    fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let sync = fixture.sync();
    drop(fixture);

    let empty = CString::new("").unwrap();
    for tag in [ptr::null(), empty.as_ptr()] {
        let mut outs = ChangeOuts::default();
        let mut error: *mut GError = ptr::null_mut();

        unsafe {
            let outcome = ops::get_changes(
                &sync,
                tag,
                GFALSE,
                &mut outs.tag,
                &mut outs.repeat,
                &mut outs.created,
                &mut outs.modified,
                &mut outs.removed,
                &mut error,
            );

            assert!(matches!(outcome, Outcome::ListInstead), "{outcome:?}");
            assert!(error.is_null(), "the fallback is not a failure");
            assert!(
                outs.tag.is_null(),
                "nothing may be written before the fallback"
            );
            assert!(outs.modified.is_null() && outs.removed.is_null());
        }
    }
}

/// RFC 8620 §5.2: a server may refuse a state it can no longer diff from. That
/// is not an error either — it is the same full listing, and reporting it as a
/// failure would strand the calendar until someone deleted the cache.
#[test]
fn a_state_the_server_cannot_diff_from_falls_back_to_a_full_listing() {
    let fixture = Fixture::start();
    fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let tag = CString::new("state-from-another-server").unwrap();

    let mut outs = ChangeOuts::default();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let outcome = ops::get_changes(
            &fixture.sync(),
            tag.as_ptr(),
            GFALSE,
            &mut outs.tag,
            &mut outs.repeat,
            &mut outs.created,
            &mut outs.modified,
            &mut outs.removed,
            &mut error,
        );

        assert!(matches!(outcome, Outcome::ListInstead), "{outcome:?}");
        assert!(error.is_null(), "the fallback is not a failure");
    }
}

// ---------------------------------------------------------------------------
// the error mapping itself

/// Each `SyncError` has to reach Evolution as the domain and code it routes on:
/// `REPOSITORY_OFFLINE` is what makes the meta backend serve its cache,
/// `OBJECT_NOT_FOUND` is what makes it drop a component, and an iCalendar we
/// cannot map is a bad argument rather than a server fault.
#[test]
fn each_sync_error_carries_the_code_evolution_routes_on() {
    let cases: Vec<(SyncError, u32, i32)> = vec![
        (
            SyncError::NotFound("K1".to_owned()),
            unsafe { e_cal_client_error_quark() },
            E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND as i32,
        ),
        (
            SyncError::Client(jmap_client::Error::Transport("down".to_owned())),
            unsafe { e_client_error_quark() },
            E_CLIENT_ERROR_REPOSITORY_OFFLINE as i32,
        ),
        (
            SyncError::ICal(jmap_ical::ICalError::NotACalendar),
            unsafe { e_client_error_quark() },
            E_CLIENT_ERROR_INVALID_ARG as i32,
        ),
        // A component we could read but cannot state as JSCalendar — a create
        // whose recurrence rule would go out as one the server may refuse. Also
        // the argument, not the server: the meta backend must report the save
        // as failed rather than shelve it for a retry that would fail the same
        // way, and the message names what the user has to change.
        (
            SyncError::Unsendable(Unsendable::Recurrence),
            unsafe { e_client_error_quark() },
            E_CLIENT_ERROR_INVALID_ARG as i32,
        ),
    ];

    for (error, domain, code) in cases {
        let mut gerror = ops::to_gerror(&error);
        // SAFETY: to_gerror hands ownership of a fresh GError over.
        unsafe { assert_error(&mut gerror, domain, code) };
    }
}

/// The sentence a user reads when a create was refused over its recurrence.
///
/// It is written here rather than in `jmap-cal-sync` for one reason: this is
/// where it can be translated. The sync layer decides *that* the save cannot go
/// out and what about it could not be stated; turning that into a sentence is a
/// user-interface act, and gettext lives on this side of the FFI. So the
/// refusal travels as a reason and is phrased at the point where the message
/// can also be looked up in the user's language — which is why the two facts
/// that identify the appointment have to survive the trip.
///
/// Untranslated here, this test's process having no catalogue: what is asserted
/// is that both facts reach the message, in a form the user can act on. The
/// instant is the one the mapping kept, so its spelling is pinned too.
#[test]
fn a_recurrence_refused_over_its_time_zone_names_the_zone_and_the_instant() {
    let failure = SyncError::Unsendable(Unsendable::RecurrenceEnd {
        until: "2026-03-31T12:00:00Z".to_owned(),
        zone: "Europe/Berlin".to_owned(),
    });

    let message = gerror_message(&failure);

    assert!(
        message.contains("Europe/Berlin"),
        "the zone the instant could not be stated in has to be named: {message}"
    );
    assert!(
        message.contains("2026-03-31T12:00:00Z"),
        "the end the user typed has to be quoted back: {message}"
    );
    assert!(
        !message.contains("%1$s") && !message.contains("%2$s"),
        "an unfilled placeholder means the user reads the template: {message}"
    );
}

/// And a refusal that is not about a time zone must not invent one.
///
/// The opposite mistake to the message above, and the more misleading of the
/// two: a rule refused for a month the `RRULE` cannot carry has no end date at
/// all, so a sentence about the calendar entry's time zone would send the user
/// to change something that is not what stopped the save.
#[test]
fn a_recurrence_refused_for_anything_else_says_nothing_about_a_time_zone() {
    let message = gerror_message(&SyncError::Unsendable(Unsendable::Recurrence));

    assert!(
        !message.contains("time zone"),
        "a refusal that is not about the zone must not mention one: {message}"
    );
    assert!(
        message.contains("repeat count"),
        "the user is owed the spelling that does work: {message}"
    );
}

/// The message of the `GError` `failure` maps to, the error freed after.
fn gerror_message(failure: &SyncError) -> String {
    let error = ops::to_gerror(failure);
    assert!(!error.is_null(), "a failure has to map to an error");
    // SAFETY: to_gerror hands ownership of a fresh GError over, and a GError's
    // message is a NUL-terminated string it owns.
    unsafe {
        let message = CStr::from_ptr((*error).message)
            .to_string_lossy()
            .into_owned();
        g_error_free(error);
        message
    }
}

// ---------------------------------------------------------------------------
// get_free_busy

/// Seconds since the epoch for the window the tests ask about —
/// 2026-09-01T00:00:00Z to 2026-09-02T00:00:00Z. Written as constants rather
/// than computed, so the test states the instant it means and
/// `marshal::utc_date` is the only thing converting.
const WINDOW_START: i64 = 1_788_220_800;
const WINDOW_END: i64 = 1_788_307_200;

/// EDS's `users` argument: a `GSList` of `gchar *` it owns. The strings are
/// kept alive by the returned `Vec`, exactly as EDS's own are alive for the
/// duration of the call.
struct Users {
    list: *mut GSList,
    _owned: Vec<CString>,
}

impl Users {
    fn new(users: &[&str]) -> Self {
        let owned: Vec<CString> = users.iter().map(|u| CString::new(*u).unwrap()).collect();
        let mut list = ptr::null_mut();
        for user in owned.iter().rev() {
            // SAFETY: `list` is a valid GSList and the pointer stays valid for
            // as long as `owned` does, which is as long as this struct.
            list = unsafe { g_slist_prepend(list, user.as_ptr() as *mut _) };
        }
        Self {
            list,
            _owned: owned,
        }
    }
}

impl Drop for Users {
    fn drop(&mut self) {
        // The nodes only, never the strings: `owned` holds those, as EDS holds
        // its own.
        unsafe { g_slist_free(self.list) };
    }
}

/// Seeds a principal that will answer for `email`.
fn seed_principal(fixture: &Fixture, email: &str) {
    let state = fixture.server.state();
    let mut state = state.lock().unwrap();
    let account = state.account_mut(&fixture.account_id).unwrap();
    account.seed_principal(jmap_proto::principals::Principal {
        principal_type: Some("individual".to_owned()),
        name: email.to_owned(),
        email: Some(email.to_owned()),
        ..Default::default()
    });
}

/// Reads the answer list the way `e_data_cal_respond_get_free_busy` does — as
/// plain strings, not as structs — and frees it.
unsafe fn take_free_busy(list: *mut GSList) -> Vec<String> {
    unsafe {
        let mut answers = Vec::new();
        for n in 0..g_slist_length(list) {
            let text = g_slist_nth_data(list, n).cast::<gchar>();
            assert!(!text.is_null(), "no node {n}");
            answers.push(CStr::from_ptr(text).to_string_lossy().into_owned());
        }
        g_slist_free_full(list, Some(g_free));
        answers
    }
}

#[test]
fn get_free_busy_reports_a_vfreebusy_per_attendee_it_could_answer_for() {
    let fixture = Fixture::start();
    seed_principal(&fixture, "bob@example.com");
    fixture.seed(&fixture.ours, "Standup", "2026-09-01T09:00:00");
    let users = Users::new(&["bob@example.com"]);
    let mut out: *mut GSList = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    let outcome = unsafe {
        ops::get_free_busy(
            &fixture.sync(),
            users.list,
            WINDOW_START,
            WINDOW_END,
            &mut out,
            &mut error,
        )
    };

    assert!(
        matches!(outcome, ops::FreeBusyOutcome::Reported),
        "{outcome:?}"
    );
    assert!(error.is_null(), "an error was set on success");
    let answers = unsafe { take_free_busy(out) };
    assert_eq!(answers.len(), 1);
    assert!(
        answers[0].contains("ATTENDEE:mailto:bob@example.com"),
        "{}",
        answers[0],
    );
    // The window EDS asked about, as the two `time_t`s converted: this is the
    // one assertion that pins `marshal::utc_date` end to end.
    assert!(
        answers[0].contains("DTSTART:20260901T000000Z"),
        "{}",
        answers[0]
    );
    assert!(
        answers[0].contains("DTEND:20260902T000000Z"),
        "{}",
        answers[0]
    );
    assert!(
        answers[0].contains("FREEBUSY;FBTYPE=BUSY:20260901T090000Z/20260901T100000Z"),
        "{}",
        answers[0],
    );
}

/// The answer that makes the vfunc chain up. Nothing may be written on this
/// path — the parent is about to write the same out-parameter, and a `GError`
/// left set would make its own `g_set_error` a critical.
#[test]
fn get_free_busy_answers_nothing_known_without_touching_the_out_parameters() {
    let fixture = Fixture::start();
    fixture.seed(&fixture.ours, "Standup", "2026-09-01T09:00:00");

    for users in [
        // Nobody asked about at all.
        Users::new(&[]),
        // Asked about, but the server has no principal for them.
        Users::new(&["stranger@example.net"]),
    ] {
        let mut out: *mut GSList = ptr::null_mut();
        let mut error: *mut GError = ptr::null_mut();

        let outcome = unsafe {
            ops::get_free_busy(
                &fixture.sync(),
                users.list,
                WINDOW_START,
                WINDOW_END,
                &mut out,
                &mut error,
            )
        };

        assert!(
            matches!(outcome, ops::FreeBusyOutcome::NothingKnown),
            "{outcome:?}",
        );
        assert!(out.is_null(), "the out-parameter was written anyway");
        assert!(error.is_null(), "an error was set anyway");
    }
}

/// A real failure sets the error and does *not* chain up, because chaining up
/// would answer the user with the account owner's cached diary and no sign
/// that the attendees they asked about were never looked up.
#[test]
fn get_free_busy_reports_a_server_failure_rather_than_falling_through() {
    let fixture = Fixture::start();
    seed_principal(&fixture, "bob@example.com");
    let sync = CalSync::new(
        fixture.client(),
        Id::new("no-such-account"),
        fixture.ours.clone(),
    );
    let users = Users::new(&["bob@example.com"]);
    let mut out: *mut GSList = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    let outcome = unsafe {
        ops::get_free_busy(
            &sync,
            users.list,
            WINDOW_START,
            WINDOW_END,
            &mut out,
            &mut error,
        )
    };

    assert!(
        matches!(outcome, ops::FreeBusyOutcome::Failed),
        "{outcome:?}"
    );
    assert!(out.is_null(), "the out-parameter was written on failure");
    assert!(!error.is_null(), "the failure was not reported");
    unsafe { g_error_free(error) };
}

/// A NULL `users` is what EDS passes for "nobody", and reading it as a list
/// would dereference it.
#[test]
fn get_free_busy_survives_a_null_user_list() {
    let fixture = Fixture::start();
    let mut error: *mut GError = ptr::null_mut();

    let outcome = unsafe {
        ops::get_free_busy(
            &fixture.sync(),
            ptr::null(),
            WINDOW_START,
            WINDOW_END,
            ptr::null_mut(),
            &mut error,
        )
    };

    assert!(
        matches!(outcome, ops::FreeBusyOutcome::NothingKnown),
        "{outcome:?}",
    );
    assert!(error.is_null());
}

/// A NULL out-parameter means "not interested", and must not be written
/// through — `set_out_list` is what keeps that true, and the answer is still
/// `Reported` because the work was done.
#[test]
fn get_free_busy_skips_an_out_parameter_the_caller_did_not_ask_for() {
    let fixture = Fixture::start();
    seed_principal(&fixture, "bob@example.com");
    fixture.seed(&fixture.ours, "Standup", "2026-09-01T09:00:00");
    let users = Users::new(&["bob@example.com"]);
    let mut error: *mut GError = ptr::null_mut();

    let outcome = unsafe {
        ops::get_free_busy(
            &fixture.sync(),
            users.list,
            WINDOW_START,
            WINDOW_END,
            ptr::null_mut(),
            &mut error,
        )
    };

    assert!(
        matches!(outcome, ops::FreeBusyOutcome::Reported),
        "{outcome:?}"
    );
    assert!(error.is_null());
}

/// `marshal::utc_date` is the only calendar arithmetic in the calendar path,
/// and it is GLib's rather than ours — these pin the two ends and the epoch
/// itself, which is where an off-by-a-timezone would show.
#[test]
fn a_time_t_becomes_the_utc_date_the_draft_asks_for() {
    assert_eq!(
        marshal::utc_date(0).as_deref(),
        Some("1970-01-01T00:00:00Z"),
    );
    assert_eq!(
        marshal::utc_date(WINDOW_START).as_deref(),
        Some("2026-09-01T00:00:00Z"),
    );
    // A leap day, which is the cheapest proof this is not string arithmetic.
    assert_eq!(
        marshal::utc_date(1_709_164_800).as_deref(),
        Some("2024-02-29T00:00:00Z"),
    );
}

/// Out of `GDateTime`'s range is `None` rather than a wrong date — the caller
/// reads that as "let the parent answer", never as an instant.
#[test]
fn a_time_t_no_calendar_can_show_is_refused_rather_than_wrapped() {
    assert_eq!(marshal::utc_date(i64::MAX), None);
    assert_eq!(marshal::utc_date(i64::MIN), None);
}

// ---------------------------------------------------------------------------
// on_source_changed

/// The colour matching the baseline is the ordinary case — every
/// `source_changed` firing that is not about the colour at all, plus every
/// firing right after this backend's own read path rewrote it — and must not
/// send anything.
#[test]
fn a_colour_matching_the_baseline_is_a_no_op() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let outcome = ops::on_source_changed(&sync, Some("#62a0ea"), Some("#62a0ea"));
    assert!(matches!(outcome, ops::ColorOutcome::Unchanged));

    let calendars = fixture.client().calendars(&fixture.account_id).unwrap();
    let ours = calendars
        .into_iter()
        .find(|c| c.id.as_ref() == Some(&fixture.ours))
        .unwrap();
    assert_eq!(ours.color, None, "nothing was ever sent");
}

/// A colour that differs from the baseline is pushed, and the outcome carries
/// the new baseline for the caller to store.
#[test]
fn a_colour_that_differs_from_the_baseline_is_pushed() {
    let fixture = Fixture::start();
    let sync = fixture.sync();

    let outcome = ops::on_source_changed(&sync, Some("#ff00ff"), Some("#62a0ea"));
    assert!(matches!(outcome, ops::ColorOutcome::Pushed(Some(ref c)) if c == "#ff00ff"));

    let calendars = fixture.client().calendars(&fixture.account_id).unwrap();
    let ours = calendars
        .into_iter()
        .find(|c| c.id.as_ref() == Some(&fixture.ours))
        .unwrap();
    assert_eq!(ours.color.as_deref(), Some("#ff00ff"));
}

/// Clearing the colour locally is a genuine difference too, and is pushed as
/// `None` rather than treated as "nothing to say".
#[test]
fn clearing_the_colour_is_pushed_as_none() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    sync.set_color(Some("#ff00ff")).unwrap();

    let outcome = ops::on_source_changed(&sync, None, Some("#ff00ff"));
    assert!(matches!(outcome, ops::ColorOutcome::Pushed(None)));

    let calendars = fixture.client().calendars(&fixture.account_id).unwrap();
    let ours = calendars
        .into_iter()
        .find(|c| c.id.as_ref() == Some(&fixture.ours))
        .unwrap();
    assert_eq!(ours.color, None);
}
