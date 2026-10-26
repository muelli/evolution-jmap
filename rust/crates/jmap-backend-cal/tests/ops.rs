// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for calendar operations and vfunc bodies called with EDS out-parameters against `jmap-mockd`.

use std::ffi::{CStr, CString};
use std::ptr;

use eds_sys::{
    E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND, E_CLIENT_ERROR_INVALID_ARG, E_CLIENT_ERROR_NOT_SUPPORTED,
    E_CLIENT_ERROR_OUT_OF_SYNC, E_CLIENT_ERROR_REPOSITORY_OFFLINE, E_CONFLICT_RESOLUTION_FAIL,
    E_CONFLICT_RESOLUTION_KEEP_LOCAL, E_CONFLICT_RESOLUTION_KEEP_SERVER,
    E_CONFLICT_RESOLUTION_USE_NEWER, E_CONFLICT_RESOLUTION_WRITE_COPY, ECalComponent,
    ECalMetaBackendInfo, EConflictResolution, ICalComponent, e_cal_client_error_quark,
    e_cal_component_get_icalcomponent, e_cal_component_new_from_string,
    e_cal_meta_backend_info_free, e_client_error_quark, i_cal_component_set_uid,
};
use glib_sys::{
    GError, GFALSE, GSList, GTRUE, g_error_free, g_free, g_slist_free, g_slist_free_full,
    g_slist_length, g_slist_nth_data, g_slist_prepend, gboolean, gchar,
};
use gobject_sys::g_object_unref;
use jmap_backend_cal::marshal;
use jmap_backend_cal::ops::{self, Outcome};
use jmap_cal_sync::{CalSync, Itip, OnConflict, SyncError, Unscheduled, Unsendable, edit_base};
use jmap_proto::Id;
use serde_json::json;

mod common;
use common::Fixture;

/// The four out-parameters EDS hands `get_changes_sync`, plus the sync tag.
struct ChangeOuts {
    tag: *mut gchar,
    repeat: gboolean,
    created: *mut GSList,
    modified: *mut GSList,
    removed: *mut GSList,
}

impl Default for ChangeOuts {
    /// Initialises repeat to TRUE to verify that the operation explicitly clears it.
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
    /// Frees created, modified, and removed lists as ECalMetaBackendInfo.
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

/// Verifies NULL out-parameters are skipped and their lists are not allocated.
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

#[test]
fn list_existing_populates_requested_out_parameters_when_others_are_null() {
    let fixture = Fixture::start();
    fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");

    let mut error: *mut GError = ptr::null_mut();
    let mut tag: *mut gchar = ptr::null_mut();

    unsafe {
        let ok = ops::list_existing(&fixture.sync(), &mut tag, ptr::null_mut(), &mut error);
        assert_eq!(ok, GTRUE);
        assert!(error.is_null());
        assert!(!take_string(&mut tag).is_empty());
    }

    let mut objects: *mut GSList = ptr::null_mut();
    unsafe {
        let ok = ops::list_existing(&fixture.sync(), ptr::null_mut(), &mut objects, &mut error);
        assert_eq!(ok, GTRUE);
        assert!(error.is_null());
        assert_eq!(g_slist_length(objects), 1);
        g_slist_free_full(objects, Some(e_cal_meta_backend_info_free));
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
            ptr::null(),
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
        let revision = fixture.sync().load_component(id.as_str()).unwrap().revision;
        assert!(
            text.contains(&format!("X-JMAP-BASE:{revision}")),
            "the component carries no stamp of its base: {text}"
        );
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
            ptr::null(),
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
            E_CONFLICT_RESOLUTION_KEEP_LOCAL,
            list,
            ptr::null(),
            ptr::null_mut(),
            Itip::Send,
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
            E_CONFLICT_RESOLUTION_KEEP_LOCAL,
            list,
            ptr::null(),
            ptr::null_mut(),
            Itip::Send,
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

/// Verifies an edit without an identifier fails instead of duplicating as a create.
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
            E_CONFLICT_RESOLUTION_KEEP_LOCAL,
            list,
            ptr::null(),
            ptr::null_mut(),
            Itip::Send,
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

/// Verifies saving instances with no master component returns an invalid argument error.
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
            E_CONFLICT_RESOLUTION_KEEP_LOCAL,
            list,
            ptr::null(),
            ptr::null_mut(),
            Itip::Send,
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
            E_CONFLICT_RESOLUTION_KEEP_LOCAL,
            ptr::null(),
            ptr::null(),
            ptr::null_mut(),
            Itip::Send,
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
            ops::remove_component(&fixture.sync(), uid.as_ptr(), Itip::Send, &mut error),
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
        let ok = ops::remove_component(&fixture.sync(), ptr::null(), Itip::Send, &mut error);
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

/// Verifies a repeat call answers from the tag just like the first call.
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

/// Verifies get_changes without a sync tag asks for a full listing without server request.
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

/// Verifies state server cannot diff from falls back to a full listing.
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

#[test]
fn get_changes_skips_unrequested_out_parameters_and_populates_requested_ones() {
    let fixture = Fixture::start();
    let doomed = fixture.seed(&fixture.ours, "Doomed Meeting", "2026-01-15T09:00:00");
    let sync = fixture.sync();
    let (state, _) = sync.list_existing().unwrap();
    let tag = CString::new(state.as_str()).unwrap();

    let created = fixture.seed(&fixture.ours, "New Meeting", "2026-01-15T10:00:00");
    sync.remove_component(doomed.as_str()).unwrap();

    let mut error: *mut GError = ptr::null_mut();

    // All out-parameters NULL.
    unsafe {
        let outcome = ops::get_changes(
            &sync,
            tag.as_ptr(),
            GFALSE,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut error,
        );
        assert!(matches!(outcome, Outcome::Reported), "{outcome:?}");
        assert!(error.is_null());
    }

    // Only sync tag and repeat requested.
    let mut new_tag: *mut gchar = ptr::null_mut();
    let mut repeat: gboolean = GTRUE;
    unsafe {
        let outcome = ops::get_changes(
            &sync,
            tag.as_ptr(),
            GFALSE,
            &mut new_tag,
            &mut repeat,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut error,
        );
        assert!(matches!(outcome, Outcome::Reported), "{outcome:?}");
        assert!(error.is_null());
        assert_eq!(repeat, GFALSE);
        assert!(!take_string(&mut new_tag).is_empty());
    }

    // Only modified list requested.
    let mut modified: *mut GSList = ptr::null_mut();
    unsafe {
        let outcome = ops::get_changes(
            &sync,
            tag.as_ptr(),
            GFALSE,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut modified,
            ptr::null_mut(),
            &mut error,
        );
        assert!(matches!(outcome, Outcome::Reported), "{outcome:?}");
        assert!(error.is_null());
        assert_eq!(g_slist_length(modified), 1);
        assert_eq!(nth_info(modified, 0).0, created.to_string());
        g_slist_free_full(modified, Some(e_cal_meta_backend_info_free));
    }

    // Only removed list requested.
    let mut removed: *mut GSList = ptr::null_mut();
    unsafe {
        let outcome = ops::get_changes(
            &sync,
            tag.as_ptr(),
            GFALSE,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut removed,
            &mut error,
        );
        assert!(matches!(outcome, Outcome::Reported), "{outcome:?}");
        assert!(error.is_null());
        assert_eq!(g_slist_length(removed), 1);
        assert_eq!(nth_info(removed, 0).0, doomed.to_string());
        g_slist_free_full(removed, Some(e_cal_meta_backend_info_free));
    }
}

// ---------------------------------------------------------------------------
// the base of an edit, which EDS keeps as the extra of a component

/// Loads `uid` through `ops::load_component` and returns the extra it stores.
fn loaded_extra(sync: &CalSync, uid: &Id) -> String {
    reloaded_extra(sync, uid, None)
}

/// [`loaded_extra`] for a refresh over the cached extra `previous`.
fn reloaded_extra(sync: &CalSync, uid: &Id, previous: Option<&str>) -> String {
    let uid = CString::new(uid.as_str()).unwrap();
    let previous = previous.map(|text| CString::new(text).unwrap());
    let mut extra: *mut gchar = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();
    unsafe {
        let ok = ops::load_component(
            sync,
            uid.as_ptr(),
            previous.as_ref().map_or(ptr::null(), |text| text.as_ptr()),
            ptr::null_mut(),
            &mut extra,
            &mut error,
        );
        assert_eq!(ok, GTRUE);
        take_string(&mut extra)
    }
}

/// The master `VEVENT` of a cached object, as EDS hands it back to a save.
fn master(icalendar: &str) -> String {
    let (_, rest) = icalendar
        .split_once("BEGIN:VEVENT")
        .expect("the object holds an event");
    let (event, _) = rest.split_once("END:VEVENT").expect("the event ends");
    format!("BEGIN:VEVENT{event}END:VEVENT\r\n")
}

/// Seeds "Standup", loads it, and has a different client add a description.
/// Returns the id and the extra of the load.
fn edited_remotely(fixture: &Fixture, sync: &CalSync, patch: serde_json::Value) -> (Id, String) {
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let extra = loaded_extra(sync, &id);
    fixture
        .client()
        .event_update(&fixture.account_id, &id, patch)
        .unwrap();
    (id, extra)
}

/// Saves `vevent` as an edit with `extra`. Returns the result, the error and the new extra.
fn save_edit(
    sync: &CalSync,
    vevent: &str,
    extra: &str,
    resolution: EConflictResolution,
    itip: Itip,
) -> (gboolean, *mut GError, String) {
    let components = [instance(vevent)];
    let list = instance_list(&components);
    let extra = CString::new(extra).unwrap();
    let mut new_uid: *mut gchar = ptr::null_mut();
    let mut new_extra: *mut gchar = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();
    unsafe {
        let ok = ops::save_component(
            sync,
            GTRUE,
            resolution,
            list,
            extra.as_ptr(),
            ptr::null_mut(),
            itip,
            &mut new_uid,
            &mut new_extra,
            &mut error,
        );
        drop_instances(list, &components);
        let new_extra = if ok == GTRUE {
            g_free(new_uid.cast());
            take_string(&mut new_extra)
        } else {
            assert!(new_extra.is_null(), "a failed save wrote an extra");
            String::new()
        };
        (ok, error, new_extra)
    }
}

#[test]
fn every_listed_node_carries_its_icalendar_as_the_extra() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let sync = fixture.sync();
    let mut sync_tag: *mut gchar = ptr::null_mut();
    let mut existing: *mut GSList = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();
    unsafe {
        let ok = ops::list_existing(&sync, &mut sync_tag, &mut existing, &mut error);
        assert_eq!(ok, GTRUE);
        let node = g_slist_nth_data(existing, 0).cast::<ECalMetaBackendInfo>();
        assert!(!(*node).extra.is_null(), "the node has no extra");
        let extra = CStr::from_ptr((*node).extra).to_string_lossy().into_owned();
        assert_eq!(extra, sync.load_component(id.as_str()).unwrap().icalendar);
        g_free(sync_tag.cast());
        g_slist_free_full(existing, Some(e_cal_meta_backend_info_free));
    }
}

#[test]
fn a_load_stores_the_icalendar_as_the_extra() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Standup", "2026-01-15T09:00:00");
    let sync = fixture.sync();
    assert_eq!(
        loaded_extra(&sync, &id),
        sync.load_component(id.as_str()).unwrap().icalendar
    );
}

/// The M2 finding: an edit of one property must not revert a remote edit of another.
#[test]
fn an_edit_with_its_extra_keeps_a_remote_edit_of_another_property() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let (id, extra) = edited_remotely(&fixture, &sync, json!({"description": "Remote notes"}));

    let edited = master(&extra).replace("SUMMARY:Standup\r\n", "SUMMARY:Standup local\r\n");
    let (ok, error, new_extra) = save_edit(
        &sync,
        &edited,
        &extra,
        E_CONFLICT_RESOLUTION_KEEP_LOCAL,
        Itip::Send,
    );

    assert_eq!(ok, GTRUE);
    assert!(error.is_null());
    let stored = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(stored.contains("SUMMARY:Standup local"), "{stored}");
    assert!(
        stored.contains("Remote notes"),
        "the remote edit was reverted: {stored}"
    );
    assert_eq!(
        edit_base::versions_of(&new_extra),
        [stored, extra],
        "the new extra is not the stored iCalendar, then the base"
    );
}

/// The E1 finding: a refresh replaced the extra while an editor kept the old copy open.
#[test]
fn a_stale_copy_keeps_a_remote_edit_after_a_refresh_replaced_the_extra() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let (id, extra) = edited_remotely(&fixture, &sync, json!({"description": "Remote notes"}));
    let refreshed = reloaded_extra(&sync, &id, Some(&extra));

    let copy = edit_base::stamp(&master(&extra), "VEVENT", &edit_base::revision_of(&extra));
    let edited = copy.replace("SUMMARY:Standup\r\n", "SUMMARY:Standup local\r\n");
    let (ok, error, _) = save_edit(
        &sync,
        &edited,
        &refreshed,
        E_CONFLICT_RESOLUTION_KEEP_LOCAL,
        Itip::Send,
    );

    assert_eq!(ok, GTRUE);
    assert!(error.is_null());
    let stored = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(stored.contains("SUMMARY:Standup local"), "{stored}");
    assert!(
        stored.contains("Remote notes"),
        "the stale copy reverted the remote edit: {stored}"
    );
    assert!(
        !stored.contains("X-JMAP-BASE"),
        "the stamp reached the server: {stored}"
    );
}

/// A copy older than every version that the extra keeps has no base, so it must not overwrite.
#[test]
fn a_stamp_that_the_extra_no_longer_keeps_is_out_of_sync() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let (id, extra) = edited_remotely(&fixture, &sync, json!({"description": "Remote notes"}));
    let refreshed = loaded_extra(&sync, &id);

    let copy = edit_base::stamp(&master(&extra), "VEVENT", &edit_base::revision_of(&extra));
    let edited = copy.replace("SUMMARY:Standup\r\n", "SUMMARY:Standup local\r\n");
    let (ok, mut error, _) = save_edit(
        &sync,
        &edited,
        &refreshed,
        E_CONFLICT_RESOLUTION_KEEP_LOCAL,
        Itip::Send,
    );

    assert_eq!(ok, GFALSE);
    unsafe {
        assert_error(
            &mut error,
            e_client_error_quark(),
            E_CLIENT_ERROR_OUT_OF_SYNC as i32,
        );
    }
    let stored = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(stored.contains("SUMMARY:Standup\r\n"), "{stored}");
    assert!(stored.contains("Remote notes"), "{stored}");
}

/// A received change comes from the organizer and replaces the event, so it has no base.
#[test]
fn a_received_change_ignores_the_extra() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let (id, extra) = edited_remotely(&fixture, &sync, json!({"description": "Remote notes"}));

    let edited = master(&extra).replace("SUMMARY:Standup\r\n", "SUMMARY:Standup moved\r\n");
    let (ok, error, _) = save_edit(
        &sync,
        &edited,
        &extra,
        E_CONFLICT_RESOLUTION_KEEP_LOCAL,
        Itip::Received { reply: false },
    );

    assert_eq!(ok, GTRUE);
    assert!(error.is_null());
    let stored = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(stored.contains("SUMMARY:Standup moved"), "{stored}");
    assert!(!stored.contains("Remote notes"), "{stored}");
}

#[test]
fn keep_server_keeps_the_remote_value_of_a_property_both_sides_changed() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let (id, extra) = edited_remotely(&fixture, &sync, json!({"title": "Standup remote"}));

    let edited = master(&extra).replace("SUMMARY:Standup\r\n", "SUMMARY:Standup local\r\n");
    let (ok, error, _) = save_edit(
        &sync,
        &edited,
        &extra,
        E_CONFLICT_RESOLUTION_KEEP_SERVER,
        Itip::Send,
    );

    assert_eq!(ok, GTRUE);
    assert!(error.is_null());
    let stored = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(stored.contains("SUMMARY:Standup remote"), "{stored}");
}

#[test]
fn fail_reports_out_of_sync_for_a_property_both_sides_changed() {
    let fixture = Fixture::start();
    let sync = fixture.sync();
    let (id, extra) = edited_remotely(&fixture, &sync, json!({"title": "Standup remote"}));

    let edited = master(&extra).replace("SUMMARY:Standup\r\n", "SUMMARY:Standup local\r\n");
    let (ok, mut error, _) = save_edit(
        &sync,
        &edited,
        &extra,
        E_CONFLICT_RESOLUTION_FAIL,
        Itip::Send,
    );

    assert_eq!(ok, GFALSE);
    unsafe {
        assert_error(
            &mut error,
            e_client_error_quark(),
            E_CLIENT_ERROR_OUT_OF_SYNC as i32,
        );
    }
    let stored = sync.load_component(id.as_str()).unwrap().icalendar;
    assert!(stored.contains("SUMMARY:Standup remote"), "{stored}");
}

#[test]
fn each_conflict_resolution_maps_to_its_on_conflict() {
    let cases = [
        (E_CONFLICT_RESOLUTION_FAIL, OnConflict::Fail),
        (E_CONFLICT_RESOLUTION_KEEP_SERVER, OnConflict::KeepServer),
        (E_CONFLICT_RESOLUTION_KEEP_LOCAL, OnConflict::KeepLocal),
        (E_CONFLICT_RESOLUTION_USE_NEWER, OnConflict::KeepLocal),
        (E_CONFLICT_RESOLUTION_WRITE_COPY, OnConflict::KeepLocal),
        (99, OnConflict::KeepLocal),
    ];
    for (resolution, expected) in cases {
        assert_eq!(ops::on_conflict(resolution), expected, "{resolution}");
    }
}

// ---------------------------------------------------------------------------
// the error mapping itself

/// Asserts `SyncError` variants map to expected GError domains and codes.
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
        // Component read but not expressible in JSCalendar (e.g. unsupported recurrence).
        (
            SyncError::Unsendable(Unsendable::Recurrence),
            unsafe { e_client_error_quark() },
            E_CLIENT_ERROR_INVALID_ARG as i32,
        ),
        // The server cannot reach a guest: the meeting is what has to change.
        (
            SyncError::Unscheduled(Unscheduled::Unreachable {
                addresses: Vec::new(),
            }),
            unsafe { e_client_error_quark() },
            E_CLIENT_ERROR_INVALID_ARG as i32,
        ),
        // The server does not schedule at all: the account's switch is wrong.
        (
            SyncError::Unscheduled(Unscheduled::NotImplemented),
            unsafe { e_client_error_quark() },
            E_CLIENT_ERROR_NOT_SUPPORTED as i32,
        ),
    ];

    for (error, domain, code) in cases {
        let mut gerror = ops::to_gerror(&error);
        // SAFETY: to_gerror hands ownership of a fresh GError over.
        unsafe { assert_error(&mut gerror, domain, code) };
    }
}

/// Verifies localized refusal message contains actionable appointment details.
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

/// A meeting the server refused to schedule names the guests it cannot reach,
/// so the user knows whom to fix, and says the change was not saved.
#[test]
fn a_guest_the_server_cannot_reach_is_named() {
    let message = gerror_message(&SyncError::Unscheduled(Unscheduled::Unreachable {
        addresses: vec!["urn:uuid:dave".to_owned(), "tel:+15555550100".to_owned()],
    }));

    assert!(
        message.contains("urn:uuid:dave, tel:+15555550100"),
        "every unreachable guest has to be named: {message}"
    );
    assert!(message.contains("not saved"), "{message}");
    assert!(!message.contains("%1$s"), "{message}");
}

/// When the sync layer could not tell which guest it was (a delete does not
/// load the event), the message says what to check instead of naming nobody.
#[test]
fn an_unnamed_unreachable_guest_still_says_what_to_check() {
    let message = gerror_message(&SyncError::Unscheduled(Unscheduled::Unreachable {
        addresses: Vec::new(),
    }));

    assert!(message.contains("not saved"), "{message}");
    assert!(message.contains("email address"), "{message}");
    assert!(!message.contains("%1$s"), "{message}");
}

/// A server that does not schedule: Evolution never sends the invitations
/// instead, so the message names no setting and offers the one way to save.
#[test]
fn a_server_that_does_not_schedule_says_to_save_without_sending() {
    let message = gerror_message(&SyncError::Unscheduled(Unscheduled::NotImplemented));

    assert!(
        !message.contains("Server handles meeting invitations"),
        "{message}"
    );
    assert!(message.contains("not saved"), "{message}");
    assert!(message.contains("choose not to send"), "{message}");
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

/// Seconds since Unix epoch for the free/busy test query window.
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
    account.seed_principal(
        jmap_proto::principals::Principal::new(email)
            .with_type("individual")
            .with_email(email),
    );
}

/// Reads and frees free/busy string list as EDS does.
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

/// Verifies get_free_busy answers NothingKnown without touching out-parameters.
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

/// Verifies get_free_busy skips NULL out-parameters.
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

/// Verifies time_t values convert correctly to UTC date format.
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

/// Verifies out-of-range timestamps return None instead of wrapping.
#[test]
fn a_time_t_no_calendar_can_show_is_refused_rather_than_wrapped() {
    assert_eq!(marshal::utc_date(i64::MAX), None);
    assert_eq!(marshal::utc_date(i64::MIN), None);
}

// ---------------------------------------------------------------------------
// on_source_changed

/// Verifies matching color baseline results in a no-op without sending requests.
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

// ---------------------------------------------------------------------------
// who tells the guests

/// A meeting the account's own identity organises, with one guest.
const MEETING: &str = "BEGIN:VEVENT\r\n\
                       UID:planning@localhost\r\n\
                       SUMMARY:Planning\r\n\
                       DTSTART:20260922T110000Z\r\n\
                       DURATION:PT1H\r\n\
                       ORGANIZER;CN=Olga:mailto:olga@example.com\r\n\
                       ATTENDEE;CN=Olga;PARTSTAT=ACCEPTED:mailto:olga@example.com\r\n\
                       ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:vera@example.com\r\n\
                       END:VEVENT\r\n";

impl Fixture {
    /// The fixture's account as Olga, the organiser of [`MEETING`], with the
    /// sync layer allowed to ask the server to schedule.
    fn scheduling_sync(&self) -> CalSync {
        {
            let state = self.server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&self.account_id).unwrap();
            account.seed_participant_identity("Olga", "mailto:olga@example.com", true);
        }
        self.sync().with_server_scheduling(true)
    }

    /// Every iTIP method the server sent, in order.
    fn sent(&self) -> Vec<String> {
        let state = self.server.state();
        let state = state.lock().unwrap();
        state
            .account(&self.account_id)
            .unwrap()
            .scheduling_outbox
            .iter()
            .map(|message| message.method.clone())
            .collect()
    }
}

/// Saves [`MEETING`] as a create through the vfunc body, with `itip`.
fn create_meeting(sync: &CalSync, itip: Itip) -> String {
    create_meeting_with_uid(sync, itip, "planning@localhost")
}

/// Saves [`MEETING`] under its own `uid` as a create, with `itip`. A second
/// create of a `UID` the calendar already holds is saved into that event, so
/// two meetings need two.
fn create_meeting_with_uid(sync: &CalSync, itip: Itip, uid: &str) -> String {
    let meeting = MEETING.replace("UID:planning@localhost", &format!("UID:{uid}"));
    let components = [instance(&meeting)];
    let list = instance_list(&components);
    let mut new_uid: *mut gchar = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a list of live components and valid out-parameters.
    unsafe {
        let ok = ops::save_component(
            sync,
            GFALSE,
            E_CONFLICT_RESOLUTION_KEEP_LOCAL,
            list,
            ptr::null(),
            ptr::null_mut(),
            itip,
            &mut new_uid,
            ptr::null_mut(),
            &mut error,
        );
        assert_eq!(ok, GTRUE);
        assert!(error.is_null());
        drop_instances(list, &components);
        take_string(&mut new_uid)
    }
}

#[test]
fn a_save_that_may_tell_the_guests_has_the_server_invite_them() {
    let fixture = Fixture::start();
    create_meeting(&fixture.scheduling_sync(), Itip::Send);
    assert_eq!(fixture.sent(), ["REQUEST"]);
}

#[test]
fn a_save_the_user_chose_not_to_send_asks_the_server_for_nothing() {
    let fixture = Fixture::start();
    create_meeting(&fixture.scheduling_sync(), Itip::Suppress);
    assert!(fixture.sent().is_empty(), "sent {:?}", fixture.sent());
}

#[test]
fn a_removal_passes_its_itip_through_to_the_server() {
    let fixture = Fixture::start();
    let sync = fixture.scheduling_sync();
    let quiet = create_meeting_with_uid(&sync, Itip::Suppress, "quiet@localhost");
    let loud = create_meeting_with_uid(&sync, Itip::Suppress, "loud@localhost");
    let mut error: *mut GError = ptr::null_mut();

    let quiet = CString::new(quiet).unwrap();
    // SAFETY: a NUL-terminated uid and a valid error out-parameter.
    unsafe {
        assert_eq!(
            ops::remove_component(&sync, quiet.as_ptr(), Itip::Suppress, &mut error),
            GTRUE
        );
    }
    assert!(fixture.sent().is_empty(), "sent {:?}", fixture.sent());

    let loud = CString::new(loud).unwrap();
    // SAFETY: as above.
    unsafe {
        assert_eq!(
            ops::remove_component(&sync, loud.as_ptr(), Itip::Send, &mut error),
            GTRUE
        );
    }
    assert_eq!(fixture.sent(), ["CANCEL"]);
}
