// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tests for `EBookMetaBackend` vfunc bodies called over `BookSync`.

use std::ffi::{CStr, CString};
use std::ptr;

use eds_sys::{
    E_BOOK_CLIENT_ERROR_CONTACT_NOT_FOUND, E_CLIENT_ERROR_INVALID_ARG, E_CLIENT_ERROR_OUT_OF_SYNC,
    E_CLIENT_ERROR_REPOSITORY_OFFLINE, E_CONFLICT_RESOLUTION_FAIL,
    E_CONFLICT_RESOLUTION_KEEP_LOCAL, E_CONFLICT_RESOLUTION_KEEP_SERVER,
    E_CONFLICT_RESOLUTION_USE_NEWER, E_CONFLICT_RESOLUTION_WRITE_COPY, EBookMetaBackendInfo,
    EConflictResolution, EContact, e_book_client_error_quark, e_book_meta_backend_info_free,
    e_client_error_quark,
};
use glib_sys::{
    GError, GFALSE, GSList, GTRUE, g_error_free, g_free, g_slist_free_full, g_slist_length,
    g_slist_nth_data, gboolean, gchar,
};
use jmap_backend_book::marshal;
use jmap_backend_book::ops::{self, Outcome};
use jmap_book_sync::{BookSync, OnConflict, SyncError, edit_base};
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
    /// `repeat` starts TRUE to verify that `get_changes` explicitly clears it.
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
    fn drop(&mut self) {
        unsafe {
            g_free(self.tag.cast());
            g_slist_free_full(self.created, Some(e_book_meta_backend_info_free));
            g_slist_free_full(self.modified, Some(e_book_meta_backend_info_free));
            g_slist_free_full(self.removed, Some(e_book_meta_backend_info_free));
        }
    }
}

/// Reads a `GSList` node as an `EBookMetaBackendInfo`, the way
/// `e_book_meta_backend_process_changes_sync` does.
unsafe fn nth_info(list: *mut GSList, n: u32) -> (String, String, String) {
    unsafe {
        let node = g_slist_nth_data(list, n).cast::<EBookMetaBackendInfo>();
        assert!(!node.is_null(), "no node {n}");
        // Empty for a NULL field rather than a dereference: `revision` and
        // `object` are documented nullable, and a removal leaves both unset.
        let text = |p: *mut gchar| {
            if p.is_null() {
                String::new()
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        };
        (
            text((*node).uid),
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

/// Asserts that a failed call set an error of exactly this domain and code,
/// and frees it. Getting the code wrong is not cosmetic: Evolution branches on
/// it, and `EBookMetaBackend` itself branches on `CONTACT_NOT_FOUND`.
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

// ---------------------------------------------------------------------------
// list_existing_sync

#[test]
fn list_existing_hands_back_one_node_per_card_in_this_book() {
    let fixture = Fixture::start();
    let mine = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    fixture.seed(&fixture.theirs, "Someone Else", "else@example.com");

    let mut tag: *mut gchar = ptr::null_mut();
    let mut objects: *mut GSList = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let ok = ops::list_existing(&fixture.sync(), &mut tag, &mut objects, &mut error);

        assert_eq!(ok, GTRUE);
        assert!(error.is_null(), "a successful call must not set an error");
        assert_eq!(g_slist_length(objects), 1, "the other book leaked in");

        let (uid, revision, object) = nth_info(objects, 0);
        assert_eq!(uid, mine.to_string());
        assert!(!revision.is_empty(), "a card needs a change token");
        assert!(object.contains("FN:Vera Oldenburg"), "{object}");

        assert!(!take_string(&mut tag).is_empty(), "no sync tag");
        g_slist_free_full(objects, Some(e_book_meta_backend_info_free));
    }
}

/// EDS reads "no objects" as a NULL list; the sync tag is still needed, or the
/// next sync has no state to go from.
#[test]
fn an_empty_book_lists_as_a_null_list_with_a_sync_tag() {
    let fixture = Fixture::start();
    fixture.seed(&fixture.theirs, "Someone Else", "else@example.com");

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

/// Unrequested NULL out-parameters are skipped and not allocated.
#[test]
fn out_parameters_the_caller_did_not_ask_for_are_skipped() {
    let fixture = Fixture::start();
    fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
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
    fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");

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
        g_slist_free_full(objects, Some(e_book_meta_backend_info_free));
    }
}

// ---------------------------------------------------------------------------
// load_contact_sync

#[test]
fn load_contact_yields_an_econtact_keyed_by_the_jmap_id() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let uid = CString::new(id.to_string()).unwrap();

    let mut contact: *mut EContact = ptr::null_mut();
    let mut extra: *mut gchar = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let ok = ops::load_contact(
            &fixture.sync(),
            uid.as_ptr(),
            ptr::null(),
            &mut contact,
            &mut extra,
            &mut error,
        );

        assert_eq!(ok, GTRUE);
        assert!(error.is_null());
        assert!(!contact.is_null(), "no contact was written");
        assert_eq!(marshal::contact_uid(contact).as_deref(), Some(id.as_str()));
        let vcard = marshal::vcard_from_contact(contact).expect("rendered");
        assert!(vcard.contains("FN:Vera Oldenburg"), "{vcard}");
        let revision = fixture.sync().load_contact(id.as_str()).unwrap().revision;
        assert!(
            vcard.contains(&format!("X-JMAP-BASE:{revision}")),
            "the contact carries no stamp of its base: {vcard}"
        );
        marshal::contact_unref(contact);
    }
}

/// Verifies contact not found returns `E_BOOK_CLIENT_ERROR_CONTACT_NOT_FOUND`.
#[test]
fn loading_an_unknown_contact_reports_contact_not_found_and_writes_nothing() {
    let fixture = Fixture::start();
    let uid = CString::new("no-such-card").unwrap();

    let mut contact: *mut EContact = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let ok = ops::load_contact(
            &fixture.sync(),
            uid.as_ptr(),
            ptr::null(),
            &mut contact,
            ptr::null_mut(),
            &mut error,
        );

        assert_eq!(ok, GFALSE);
        assert!(contact.is_null(), "a failed load must leave the out NULL");
        assert_error(
            &mut error,
            e_book_client_error_quark(),
            E_BOOK_CLIENT_ERROR_CONTACT_NOT_FOUND as i32,
        );
    }
}

// ---------------------------------------------------------------------------
// save_contact_sync

#[test]
fn saving_a_new_contact_creates_it_under_the_identifier_the_server_assigns() {
    let fixture = Fixture::start();
    // What Evolution hands a backend for a brand-new contact: a uid it
    // invented locally, which is not a JMAP id and must not become one.
    let contact = marshal::contact_from_vcard(
        "BEGIN:VCARD\r\n\
         VERSION:3.0\r\n\
         UID:pas-id-6890AB\r\n\
         FN:Vera Oldenburg\r\n\
         EMAIL:vera@example.com\r\n\
         END:VCARD\r\n",
    );
    assert!(!contact.is_null());

    let mut new_uid: *mut gchar = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let ok = ops::save_contact(
            &fixture.sync(),
            GFALSE,
            E_CONFLICT_RESOLUTION_KEEP_LOCAL,
            contact,
            ptr::null(),
            &mut new_uid,
            ptr::null_mut(),
            &mut error,
        );

        assert_eq!(ok, GTRUE);
        assert!(error.is_null());
        let assigned = take_string(&mut new_uid);
        assert_ne!(
            assigned, "pas-id-6890AB",
            "the local uid reached the server"
        );
        assert_eq!(fixture.uids(), vec![assigned]);
        marshal::contact_unref(contact);
    }
}

#[test]
fn saving_an_existing_contact_patches_it_rather_than_adding_a_second() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();
    let edited = sync
        .load_contact(id.as_str())
        .unwrap()
        .vcard
        .replace("FN:Vera Oldenburg", "FN:Vera Oldenburg-Meier");
    let contact = marshal::contact_from_vcard(&edited);
    assert!(!contact.is_null());

    let mut new_uid: *mut gchar = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let ok = ops::save_contact(
            &sync,
            GTRUE,
            E_CONFLICT_RESOLUTION_KEEP_LOCAL,
            contact,
            ptr::null(),
            &mut new_uid,
            ptr::null_mut(),
            &mut error,
        );

        assert_eq!(ok, GTRUE);
        assert_eq!(take_string(&mut new_uid), id.to_string());
        assert_eq!(fixture.uids(), vec![id.to_string()], "a duplicate was made");
        assert!(
            sync.load_contact(id.as_str())
                .unwrap()
                .vcard
                .contains("FN:Vera Oldenburg-Meier")
        );
        marshal::contact_unref(contact);
    }
}

/// An edit whose contact carries no identifier would otherwise be sent as a
/// create, which silently duplicates the user's contact on the server. A
/// visible failure is the better answer.
#[test]
fn an_edit_without_an_identifier_is_refused_rather_than_duplicating() {
    let fixture = Fixture::start();
    let contact =
        marshal::contact_from_vcard("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Nobody\r\nEND:VCARD\r\n");
    assert!(!contact.is_null());

    let mut new_uid: *mut gchar = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let ok = ops::save_contact(
            &fixture.sync(),
            GTRUE,
            E_CONFLICT_RESOLUTION_KEEP_LOCAL,
            contact,
            ptr::null(),
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
        assert!(fixture.uids().is_empty(), "the book was written to anyway");
        marshal::contact_unref(contact);
    }
}

#[test]
fn saving_no_contact_at_all_is_an_invalid_argument_not_a_panic() {
    let fixture = Fixture::start();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let ok = ops::save_contact(
            &fixture.sync(),
            GFALSE,
            E_CONFLICT_RESOLUTION_KEEP_LOCAL,
            ptr::null_mut(),
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
// remove_contact_sync

#[test]
fn removing_a_contact_destroys_it_on_the_server() {
    let fixture = Fixture::start();
    let doomed = fixture.seed(&fixture.ours, "Ines Tollow", "ines@example.com");
    let kept = fixture.seed(&fixture.ours, "Ada Reinsch", "ada@example.com");
    let uid = CString::new(doomed.to_string()).unwrap();

    let mut error: *mut GError = ptr::null_mut();
    unsafe {
        assert_eq!(
            ops::remove_contact(&fixture.sync(), uid.as_ptr(), &mut error),
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
        let ok = ops::remove_contact(&fixture.sync(), ptr::null(), &mut error);
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
fn get_changes_reports_changed_cards_and_the_ones_that_are_gone() {
    let fixture = Fixture::start();
    let doomed = fixture.seed(&fixture.ours, "Ines Tollow", "ines@example.com");
    let sync = fixture.sync();
    let (state, _) = sync.list_existing().unwrap();
    let tag = CString::new(state.as_str()).unwrap();

    let created = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    sync.remove_contact(doomed.as_str()).unwrap();

    let mut outs = ChangeOuts::default();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let outcome = ops::get_changes(
            &sync,
            tag.as_ptr(),
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
        // Read as an `EBookMetaBackendInfo`.
        assert_eq!(nth_info(outs.removed, 0).0, doomed.to_string());
    }
}

/// When called without a sync tag (NULL or empty string), `get_changes` returns `Outcome::ListInstead`.
#[test]
fn get_changes_without_a_sync_tag_asks_for_a_full_listing_without_asking_the_server() {
    let fixture = Fixture::start();
    fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
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

/// When the server rejects an expired state, `get_changes` falls back to `Outcome::ListInstead`.
#[test]
fn a_state_the_server_cannot_diff_from_falls_back_to_a_full_listing() {
    let fixture = Fixture::start();
    fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let tag = CString::new("state-from-another-server").unwrap();

    let mut outs = ChangeOuts::default();
    let mut error: *mut GError = ptr::null_mut();

    unsafe {
        let outcome = ops::get_changes(
            &fixture.sync(),
            tag.as_ptr(),
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
    let doomed = fixture.seed(&fixture.ours, "Ines Tollow", "ines@example.com");
    let sync = fixture.sync();
    let (state, _) = sync.list_existing().unwrap();
    let tag = CString::new(state.as_str()).unwrap();

    let created = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    sync.remove_contact(doomed.as_str()).unwrap();

    let mut error: *mut GError = ptr::null_mut();

    // All out-parameters NULL.
    unsafe {
        let outcome = ops::get_changes(
            &sync,
            tag.as_ptr(),
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
        g_slist_free_full(modified, Some(e_book_meta_backend_info_free));
    }

    // Only removed list requested.
    let mut removed: *mut GSList = ptr::null_mut();
    unsafe {
        let outcome = ops::get_changes(
            &sync,
            tag.as_ptr(),
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
        g_slist_free_full(removed, Some(e_book_meta_backend_info_free));
    }
}

// ---------------------------------------------------------------------------
// the base of an edit, which EDS keeps as the extra of a contact

/// The `extra` of node `n`, or an empty string for NULL.
unsafe fn nth_extra(list: *mut GSList, n: u32) -> String {
    unsafe {
        let node = g_slist_nth_data(list, n).cast::<EBookMetaBackendInfo>();
        assert!(!node.is_null(), "no node {n}");
        let extra = (*node).extra;
        if extra.is_null() {
            String::new()
        } else {
            CStr::from_ptr(extra).to_string_lossy().into_owned()
        }
    }
}

/// Loads `uid` through `ops::load_contact` and returns the extra it stores.
fn loaded_extra(sync: &BookSync, uid: &Id) -> String {
    reloaded_extra(sync, uid, None)
}

/// [`loaded_extra`] for a refresh over the cached extra `previous`.
fn reloaded_extra(sync: &BookSync, uid: &Id, previous: Option<&str>) -> String {
    let uid = CString::new(uid.as_str()).unwrap();
    let previous = previous.map(|text| CString::new(text).unwrap());
    let mut extra: *mut gchar = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();
    unsafe {
        let ok = ops::load_contact(
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

/// Saves `vcard` as an edit of `extra` with `resolution`. Returns the result,
/// the error and the new extra.
fn save_edit(
    sync: &BookSync,
    vcard: &str,
    extra: &str,
    resolution: EConflictResolution,
) -> (gboolean, *mut GError, String) {
    let contact = marshal::contact_from_vcard(vcard);
    assert!(!contact.is_null());
    let extra = CString::new(extra).unwrap();
    let mut new_uid: *mut gchar = ptr::null_mut();
    let mut new_extra: *mut gchar = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();
    unsafe {
        let ok = ops::save_contact(
            sync,
            GTRUE,
            resolution,
            contact,
            extra.as_ptr(),
            &mut new_uid,
            &mut new_extra,
            &mut error,
        );
        marshal::contact_unref(contact);
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
fn every_listed_node_carries_its_vcard_as_the_extra() {
    let fixture = Fixture::start();
    fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let mut sync_tag: *mut gchar = ptr::null_mut();
    let mut existing: *mut GSList = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();
    unsafe {
        let ok = ops::list_existing(&fixture.sync(), &mut sync_tag, &mut existing, &mut error);
        assert_eq!(ok, GTRUE);
        let (_, revision, object) = nth_info(existing, 0);
        assert!(object.contains("FN:Vera Oldenburg"), "{object}");
        // The object carries the stamp of its base; the extra does not.
        assert_eq!(
            edit_base::stamp(&nth_extra(existing, 0), "VCARD", &revision),
            object
        );
        g_free(sync_tag.cast());
        g_slist_free_full(existing, Some(e_book_meta_backend_info_free));
    }
}

#[test]
fn a_load_stores_the_vcard_as_the_extra() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();
    assert_eq!(
        loaded_extra(&sync, &id),
        sync.load_contact(id.as_str()).unwrap().vcard
    );
}

/// The M2 finding: an edit of one property must not revert a remote edit of another.
#[test]
fn an_edit_with_its_extra_keeps_a_remote_edit_of_another_property() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();
    let extra = loaded_extra(&sync, &id);
    fixture
        .client()
        .contact_update(
            &fixture.account_id,
            &id,
            json!({"emails/e0/address": "vera@remote.example"}),
        )
        .unwrap();

    let edited = extra.replace("FN:Vera Oldenburg", "FN:Vera Local");
    let (ok, error, new_extra) =
        save_edit(&sync, &edited, &extra, E_CONFLICT_RESOLUTION_KEEP_LOCAL);

    assert_eq!(ok, GTRUE);
    assert!(error.is_null());
    let stored = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(stored.contains("FN:Vera Local"), "{stored}");
    assert!(
        stored.contains("vera@remote.example"),
        "the remote edit was reverted: {stored}"
    );
    assert_eq!(
        edit_base::versions_of(&new_extra),
        [stored, extra],
        "the new extra is not the stored vCard, then the base"
    );
}

/// The E1 finding: a refresh replaced the extra while an editor kept the old copy open.
#[test]
fn a_stale_copy_keeps_a_remote_edit_after_a_refresh_replaced_the_extra() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();
    let extra = loaded_extra(&sync, &id);
    fixture
        .client()
        .contact_update(
            &fixture.account_id,
            &id,
            json!({"emails/e0/address": "vera@remote.example"}),
        )
        .unwrap();
    let refreshed = reloaded_extra(&sync, &id, Some(&extra));

    let copy = edit_base::stamp(&extra, "VCARD", &edit_base::revision_of(&extra));
    let edited = copy.replace("FN:Vera Oldenburg", "FN:Vera Local");
    let (ok, error, _) = save_edit(&sync, &edited, &refreshed, E_CONFLICT_RESOLUTION_KEEP_LOCAL);

    assert_eq!(ok, GTRUE);
    assert!(error.is_null());
    let stored = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(stored.contains("FN:Vera Local"), "{stored}");
    assert!(
        stored.contains("vera@remote.example"),
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
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();
    let extra = loaded_extra(&sync, &id);
    fixture
        .client()
        .contact_update(
            &fixture.account_id,
            &id,
            json!({"emails/e0/address": "vera@remote.example"}),
        )
        .unwrap();
    let refreshed = loaded_extra(&sync, &id);

    let copy = edit_base::stamp(&extra, "VCARD", &edit_base::revision_of(&extra));
    let edited = copy.replace("FN:Vera Oldenburg", "FN:Vera Local");
    let (ok, mut error, _) =
        save_edit(&sync, &edited, &refreshed, E_CONFLICT_RESOLUTION_KEEP_LOCAL);

    assert_eq!(ok, GFALSE);
    unsafe {
        assert_error(
            &mut error,
            e_client_error_quark(),
            E_CLIENT_ERROR_OUT_OF_SYNC as i32,
        );
    }
    let stored = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(stored.contains("FN:Vera Oldenburg"), "{stored}");
    assert!(stored.contains("vera@remote.example"), "{stored}");
}

#[test]
fn keep_server_keeps_the_remote_value_of_a_property_both_sides_changed() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();
    let extra = loaded_extra(&sync, &id);
    fixture
        .client()
        .contact_update(
            &fixture.account_id,
            &id,
            json!({"emails/e0/address": "vera@remote.example"}),
        )
        .unwrap();

    let edited = extra.replace("vera@example.com", "vera@local.example");
    let (ok, error, _) = save_edit(&sync, &edited, &extra, E_CONFLICT_RESOLUTION_KEEP_SERVER);

    assert_eq!(ok, GTRUE);
    assert!(error.is_null());
    let stored = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(stored.contains("vera@remote.example"), "{stored}");
}

#[test]
fn fail_reports_out_of_sync_for_a_property_both_sides_changed() {
    let fixture = Fixture::start();
    let id = fixture.seed(&fixture.ours, "Vera Oldenburg", "vera@example.com");
    let sync = fixture.sync();
    let extra = loaded_extra(&sync, &id);
    fixture
        .client()
        .contact_update(
            &fixture.account_id,
            &id,
            json!({"emails/e0/address": "vera@remote.example"}),
        )
        .unwrap();

    let edited = extra.replace("vera@example.com", "vera@local.example");
    let (ok, mut error, _) = save_edit(&sync, &edited, &extra, E_CONFLICT_RESOLUTION_FAIL);

    assert_eq!(ok, GFALSE);
    unsafe {
        assert_error(
            &mut error,
            e_client_error_quark(),
            E_CLIENT_ERROR_OUT_OF_SYNC as i32,
        );
    }
    let stored = sync.load_contact(id.as_str()).unwrap().vcard;
    assert!(stored.contains("vera@remote.example"), "{stored}");
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

#[test]
fn on_conflict_shares_core_mapping() {
    let core_fn = jmap_backend_core::marshal::on_conflict as *const () as usize;
    let ops_fn = ops::on_conflict as *const () as usize;
    assert_eq!(ops_fn, core_fn);
}

// ---------------------------------------------------------------------------
// the error mapping itself

/// Verifies `SyncError` variants map to expected `GError` domains and codes.
#[test]
fn each_sync_error_carries_the_code_evolution_routes_on() {
    let cases: Vec<(SyncError, u32, i32)> = vec![
        (
            SyncError::NotFound("K1".to_owned()),
            unsafe { e_book_client_error_quark() },
            E_BOOK_CLIENT_ERROR_CONTACT_NOT_FOUND as i32,
        ),
        (
            SyncError::Client(jmap_client::Error::Transport("down".to_owned())),
            unsafe { e_client_error_quark() },
            E_CLIENT_ERROR_REPOSITORY_OFFLINE as i32,
        ),
        (
            SyncError::VCard(jmap_vcard::VCardError::NotAVCard),
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
