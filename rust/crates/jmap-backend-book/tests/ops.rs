// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `EBookMetaBackend` vfunc bodies, called the way EDS calls them:
//! out-parameters that start NULL, a `GError **` that starts NULL, and a
//! return value that says which of the two was written.
//!
//! Every test runs against `jmap-mockd`, so the assertions are about what the
//! server was actually told, not about what the code meant to say. What is
//! deliberately *not* here is a live `EBookMetaBackend` instance: constructing
//! one needs an `ESourceRegistry`, which needs a running
//! `evolution-source-registry` on the session bus. Keeping the vfunc bodies in
//! a layer that takes a `&BookSync` is what lets them be tested at all.

use std::ffi::{CStr, CString};
use std::ptr;

use eds_sys::{
    E_BOOK_CLIENT_ERROR_CONTACT_NOT_FOUND, E_CLIENT_ERROR_INVALID_ARG,
    E_CLIENT_ERROR_REPOSITORY_OFFLINE, EBookMetaBackendInfo, EContact, e_book_client_error_quark,
    e_book_meta_backend_info_free, e_client_error_quark,
};
use glib_sys::{
    GError, GFALSE, GSList, GTRUE, g_error_free, g_free, g_slist_free_full, g_slist_length,
    g_slist_nth_data, gboolean, gchar,
};
use jmap_backend_book::marshal;
use jmap_backend_book::ops::{self, Outcome};
use jmap_book_sync::{BookSync, SyncError};
use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::contacts::ContactCard;

/// A mock server with two address books, so "only this book" stays
/// observable, and the `BookSync` over the one the backend syncs.
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
                account.seed_address_book("Personal", true),
                account.seed_address_book("Shared", false),
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

    fn sync(&self) -> BookSync {
        BookSync::new(self.client(), self.account_id.clone(), self.ours.clone())
    }

    /// Create a card directly, bypassing the code under test.
    fn seed(&self, book: &Id, full_name: &str, email: &str) -> Id {
        self.client()
            .contact_create(
                &self.account_id,
                &ContactCard::simple(book.clone(), full_name, email),
            )
            .unwrap()
            .id
            .expect("server assigned id")
    }

    /// The uids the book holds, as the server sees them.
    fn uids(&self) -> Vec<String> {
        let (_, contacts) = self.sync().list_existing().unwrap();
        let mut uids: Vec<String> = contacts.into_iter().map(|info| info.uid).collect();
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
    /// `repeat` starts TRUE, which is *not* what EDS does — it passes a
    /// FALSE it initialised itself. Starting from the other value is what
    /// makes "the answer is always no" an assertion rather than a
    /// coincidence: a body that never writes the parameter would otherwise
    /// look identical to one that answers correctly.
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

/// A NULL out-parameter is GLib's "the caller does not want this one". It has
/// to be skipped rather than written through, and the list it would have held
/// must not be built at all — there would be nobody to free it.
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
        marshal::contact_unref(contact);
    }
}

/// `EBookMetaBackend` matches on this exact domain and code to decide that a
/// card is gone rather than that the sync failed, so a not-found reported any
/// other way is a cache entry that never goes away.
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
            contact,
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
            contact,
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
            contact,
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
        // Read as an `EBookMetaBackendInfo`, which is what
        // `e_book_meta_backend_process_changes_sync` does with this list —
        // reading it as a bare string instead is the shape that crashed the
        // address book factory in `sqlite3_vmprintf`.
        assert_eq!(nth_info(outs.removed, 0).0, doomed.to_string());
    }
}

/// The first sync has no tag to go from. Answering it with an empty delta
/// would leave the address book permanently empty, so the meta backend's own
/// implementation — list the book and diff it against the cache — has to run.
///
/// The server is stopped first, which is what makes this an assertion rather
/// than a coincidence: an absent tag sent on as an empty `sinceState` would
/// come back a transport failure, and a server that happened to reject the
/// empty state would otherwise produce the same fallback for the wrong reason.
///
/// Both spellings of "absent" are checked. The EDS cache writes NULL, but an
/// empty string reaches the same place through a hand-edited cache — and `""`
/// handed back as a `sinceState` is a state, not the absence of one.
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

/// RFC 8620 §5.2: a server may refuse a state it can no longer diff from. That
/// is not an error either — it is the same full listing, and reporting it as a
/// failure would strand the account until someone deleted the cache.
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

// ---------------------------------------------------------------------------
// the error mapping itself

/// Each `SyncError` has to reach Evolution as the domain and code it routes
/// on: `REPOSITORY_OFFLINE` is what makes the meta backend serve its cache,
/// `CONTACT_NOT_FOUND` is what makes it drop a card, and a vCard we cannot map
/// is a bad argument rather than a server fault.
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
