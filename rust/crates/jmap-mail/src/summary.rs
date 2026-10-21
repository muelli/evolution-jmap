// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelFolderSummary`: the object that keeps the rows that
//! [`crate::message_info`] builds for a folder.
//!
//! A `CamelFolder` alone is only a name and a store. The summary holds the
//! contents of the folder. Camel asks the summary for the message count of the
//! folder, for the uid list of the message list, and for the row of each line.
//! Camel asks nothing of a folder that has no summary. Camel tests the
//! `CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY` flag first, and [`crate::folder`] can
//! set that flag only because this summary exists.
//!
//! ## The subclass
//!
//! `CamelJmapSummary` overrides two vfuncs. Usually, a `CamelFolderSummary`
//! subclass builds rows from *messages*. The three `message_info_new_from_*`
//! vfuncs make a row from a parser, a MIME message or a header list, and
//! `next_uid_string` makes a uid for a message that has no uid. This provider
//! does not use those four paths. It does not parse a JMAP folder: the rows
//! come from `Email/get` as structured data, and each row has the immutable id
//! of the server.
//!
//! The first purpose of the subclass is a value that is not a vfunc:
//! `message_info_type`, the class of each row that Camel makes. Camel reads
//! that field when it loads the summary from the database. Without it, a folder
//! has plain `CamelMessageInfoBase` rows after a restart. These rows have no
//! [`server keywords`], and without that column a flag change is a guess, not a
//! difference. The rows that [`crate::message_info`] builds always have the
//! subclass type. This field gives the same type to the rows that Camel builds.
//!
//! [`server keywords`]: crate::message_info::server_keywords
//!
//! ## The state of the rows
//!
//! The two overridden vfuncs are `summary_header_save` and
//! `summary_header_load`. They save and load one string: the `Email` state of
//! the last listing of this mailbox. The state is a fact about the rows (it
//! tells which state they agree with). Thus the summary keeps it, not the
//! folder, and the state is stored and loaded together with the rows. A state
//! kept only in memory is lost at a restart. That case is the most important:
//! without a stored state, the first refresh of each session lists every
//! mailbox in full.
//!
//! Camel keeps one header record per folder next to the rows. The record has a
//! `bdata` field for provider data that Camel does not have. The message row
//! has a `bdata` field too, and [`crate::message_info`] keeps the keywords in
//! it. One difference controls the format below. All classes in the chain write
//! and read the `bdata` of a row through one shared cursor. The `bdata` of a
//! header has no shared cursor: `summary_header_load` gets only the record.
//! Thus the field belongs to the last class in the chain, and this class writes
//! all of the field. It does not append to it.
//!
//! The record is `CamelFIRecord` up to EDS 3.52 and `CamelStoreDBFolderRecord`
//! from 3.58. This module names it [`eds_sys::compat::CamelSummaryFolderRecord`].
//! EDS 3.58 also changed the *signature* of `summary_header_save` (this did not
//! occur for the message row). Before, the vfunc allocates a record and returns
//! it. After, the vfunc fills the record of the caller and reports success.
//! Thus this module has two versions of that vfunc. Both use one function that
//! writes the field.
//!
//! The field starts with a format number before the state. This is for an
//! upgrade, not for a restart. This version must not read a header from a
//! later version of this provider as a state. When the number is not known,
//! the summary has no state. The cost is one full listing, which is the
//! current behaviour of a folder without a state. A misread field can cost
//! the mailbox.
//!
//! ## A listing is not a copy
//!
//! Each refresh of the folder calls [`apply_listing`], so most of the rows that
//! it gets are already in the summary. Three rules apply, and the tests examine
//! them:
//!
//! - When the listing does not name a message, the message is not in the
//!   mailbox, and its row is removed. JMAP moves mail through a change of
//!   `mailboxIds`, so inside one folder a move and a delete look the same.
//! - When a message has a row, the row stays and only its mutable columns
//!   change. RFC 8621 §4.1 makes all properties except `keywords` and
//!   `mailboxIds` immutable, so a refresh cannot get other new data. A new row
//!   would also delete the local marks that Camel keeps in the same flags word.
//! - A new message gets a row with the id of the server, not a uid from the
//!   summary.
//!
//! ## A delta is not a listing
//!
//! [`apply_delta`] is the other path for rows. It is necessary because the
//! first rule above is incorrect for a delta. `Email/changes` gives data for the
//! *account*: it tells what changed, not what is in the mailbox. A row that the
//! delta does not name has no new data. If [`apply_listing`] got a delta with
//! one new message, it would remove all other rows of the folder.
//!
//! Thus the two functions write a row in the same way, but a row that is not
//! named has a different meaning. A delta tells which messages this mailbox
//! holds now (`present`) and which it does not hold (`absent`). All other rows
//! stay as they are. A delta also gives data that a listing cannot give (see
//! [`crate::changes`]). When the delta names a message that has no row, the
//! message arrived after the state that the folder recorded. That is the
//! meaning of recent.

use std::collections::BTreeSet;
use std::ffi::CStr;
use std::ptr;
use std::sync::Mutex;

use eds_sys::compat::{CamelSummaryFolderRecord, summary_dup_uids, summary_free_uids};
use eds_sys::{
    CamelFolder, CamelFolderSummary, CamelFolderSummaryClass, camel_folder_summary_add,
    camel_folder_summary_check_uid, camel_folder_summary_count, camel_folder_summary_get,
    camel_folder_summary_get_type, camel_folder_summary_load, camel_folder_summary_lock,
    camel_folder_summary_remove_uid, camel_folder_summary_touch, camel_folder_summary_unlock,
    camel_folder_take_folder_summary, camel_util_bdata_get_number, camel_util_bdata_get_string,
    camel_util_bdata_put_number, camel_util_bdata_put_string,
};
use glib_sys::{
    GError, GFALSE, GTRUE, GType, g_free, g_string_free, g_string_new, gboolean, gchar,
};
use gobject_sys::g_object_new;
use jmap_backend_core::instance::Slot;
use jmap_backend_core::marshal::checked_borrow;
use jmap_backend_core::owned::Owned;
use jmap_backend_core::subclass::{self, ObjectSubclass, register_static};
use jmap_backend_core::trampoline::{guard, log_critical};
use jmap_mail_sync::MessageSummary;
use jmap_proto::{Id, State};

use crate::changes::Changes;
use crate::folder_info::c_string;
use crate::message_info::{
    clear_pending_write, message_info_type, new_message_info, update_message_info,
};

/// The format number of the stored header. Increase it when the meaning of the
/// fields after it changes. A header with a different number gives a summary
/// with no state, the same as a folder that has never listed.
const HEADER_VERSION: i64 = 1;

/// The instance struct: the summary of Camel, plus the one fact about its rows
/// that Camel has no field for.
#[repr(C)]
pub struct JmapSummary {
    parent: CamelFolderSummary,
    /// The `Email` state of the listing that gave these rows. `None` for a
    /// summary that has never listed, so the next refresh lists and does not
    /// ask for a delta from an invented state.
    ///
    /// A [`Slot`], because the struct starts zeroed and no destructor runs when
    /// it is freed. A [`Mutex`], because each refresh writes it and Camel uses a
    /// folder from more than one thread.
    state: Slot<Mutex<Option<State>>>,
}

impl JmapSummary {
    /// The Rust view of a summary from Camel, or `None` if the summary is not
    /// of this type.
    ///
    /// The type is checked for the same reason that [`crate::message_info`]
    /// checks a row: a folder that existed before this type was registered has
    /// a plain `CamelFolderSummary`.
    ///
    /// # Safety
    ///
    /// `summary` must be NULL or point at a live `CamelFolderSummary`.
    unsafe fn borrow<'a>(summary: *mut CamelFolderSummary) -> Option<&'a Self> {
        // SAFETY: the contract above makes the type check legal, and the check
        // makes the cast inside sound.
        unsafe { checked_borrow(summary, summary_type()) }
    }
}

/// The class struct. It adds no field to the parent class; `class_init` sets
/// the parent's `message_info_type`, which is the purpose of this type.
#[repr(C)]
pub struct JmapSummaryClass {
    parent_class: CamelFolderSummaryClass,
}

// SAFETY: both structs are #[repr(C)] and start with the CamelFolderSummary
// instance and class structs, whose layouts eds-sys's tests/layout.rs checks
// against `g_type_query`; CamelFolderSummary derives from GObject.
unsafe impl ObjectSubclass for JmapSummary {
    const NAME: &'static CStr = c"CamelJmapSummary";
    type Instance = JmapSummary;
    type Class = JmapSummaryClass;

    fn parent_type() -> GType {
        // SAFETY: the function has no arguments, and the type initialises
        // itself.
        unsafe { camel_folder_summary_get_type() }
    }

    fn class_init_types() -> Vec<GType> {
        // Register the row type before this type, not in its class initialiser.
        // `class_init` runs under the class-initialisation lock of GLib, and
        // there it cannot get the registration lock without inverting the two.
        vec![message_info_type()]
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: the class starts with CamelFolderSummaryClass, by the
        // contract above.
        let class = unsafe { &mut (*class).parent_class };
        // Camel makes a row of this type each time it builds a row itself. After
        // a restart, that is every row of every folder, from the summary
        // database.
        class.message_info_type = message_info_type();
        class.summary_header_save = Some(summary_header_save);
        class.summary_header_load = Some(summary_header_load);
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // Fill the slot here. Then an unlisted summary gives `None` because its
        // state is empty, not because the slot is empty. An empty slot is a bug
        // that looks like an empty state.
        //
        // SAFETY: the instance is being constructed, so this is the only
        // reference to it.
        unsafe { (*instance).state.init(Mutex::new(None)) };
    }

    unsafe fn finalize(instance: *mut Self::Instance) {
        // SAFETY: the instance is being finalized, so nothing can still reach
        // it and no borrow from `get` is alive.
        unsafe { (*instance).state.clear() };
    }
}

/// The `Email` state of the rows of `summary`. `None` if the summary has never
/// listed, or if it is not of this type.
///
/// This returns a copy, not a borrow, for the same reason as the keywords of a
/// row. The value is behind a mutex that gives out no references, so a refresh
/// that changes it cannot occur while a reader uses it.
///
/// # Safety
///
/// `summary` must be NULL or point at a live `CamelFolderSummary`.
pub unsafe fn summary_state(summary: *mut CamelFolderSummary) -> Option<State> {
    // SAFETY: the contract above, and `borrow` checks the type.
    let summary = unsafe { JmapSummary::borrow(summary) }?;
    let state = summary
        .state
        .get()?
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.clone()
}

/// The number of rows in `summary`. A refresh uses this number to decide how
/// much of the mailbox to fetch.
///
/// Camel counts in `guint`. The count is used only as a cost estimate against a
/// number of messages, so a count that does not fit in `usize` becomes
/// `usize::MAX`. For a mailbox that large, the estimate is "very large" in all
/// cases.
///
/// Zero for a NULL summary. This is not a special case: a folder with no rows
/// gets no benefit from a delta.
///
/// # Safety
///
/// `summary` must be NULL or point at a live `CamelFolderSummary`.
pub unsafe fn summary_rows(summary: *mut CamelFolderSummary) -> usize {
    if summary.is_null() {
        return 0;
    }
    // SAFETY: a live summary by the contract above; the accessor takes the
    // summary's own lock.
    let count = unsafe { camel_folder_summary_count(summary) };
    usize::try_from(count).unwrap_or(usize::MAX)
}

/// Records the state of one listing, and replaces the state that the summary
/// had.
///
/// It touches the summary when the state changes. Camel does not save a
/// summary that is not marked as changed, and a refresh with no new mail
/// changes nothing else. Without the touch, the state is lost with the folder.
///
/// It does nothing for a summary that is not of this type. The result is the
/// same as `None`: a folder that cannot keep a state does a full listing.
///
/// # Safety
///
/// `summary` must be NULL or point at a live `CamelFolderSummary`.
pub unsafe fn set_summary_state(summary: *mut CamelFolderSummary, state: State) {
    // SAFETY: the contract above, and `borrow` checks the type.
    let Some(borrowed) = (unsafe { JmapSummary::borrow(summary) }) else {
        return;
    };
    let Some(slot) = borrowed.state.get() else {
        return;
    };
    {
        let mut slot = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.as_ref() == Some(&state) {
            // This is the usual result for a mailbox with no writes. If this
            // marked the summary as changed, Camel would write the database at
            // each poll of each folder.
            return;
        }
        *slot = Some(state);
    }
    // SAFETY: the summary is live and of this type, by the borrow above.
    unsafe { camel_folder_summary_touch(summary) };
}

/// `CamelFolderSummaryClass.summary_header_save`, up to EDS 3.52: writes the
/// state into the record that Camel stores, and returns the record.
///
/// The parent allocates the record (this function chains up), so each count
/// and flag in it comes from the class that owns it. Only `bdata` belongs to
/// this class. [`store_state`] writes it, and the EDS 3.58 version below uses
/// it too.
#[cfg(camel_summary_records)]
unsafe extern "C" fn summary_header_save(
    summary: *mut CamelFolderSummary,
    error: *mut *mut GError,
) -> *mut CamelSummaryFolderRecord {
    guard_summary("summary_header_save", summary, ptr::null_mut(), || {
        // SAFETY: chaining up to the parent's own save, on an instance of a
        // type derived from it, with the arguments Camel passed through
        // unchanged.
        let record = unsafe { parent_class() }
            .and_then(|class| class.summary_header_save)
            .map_or(ptr::null_mut(), |save| unsafe { save(summary, error) });
        if record.is_null() {
            return record;
        }

        // SAFETY: `summary` is of this type, by the guard; the parent just
        // allocated the record, so nothing else holds its `bdata`.
        unsafe { store_state(summary, record) };
        record
    })
}

/// The same vfunc from EDS 3.58. Here it fills a record that the *caller* owns,
/// and reports success.
///
/// This is the only vfunc in the provider where a newer EDS changed the
/// signature and not only a name, so it needs two functions, not an alias. In
/// 3.52, the implementation allocates a `CamelFIRecord` and returns it. In 3.58,
/// `camel_folder_summary_save` zeroes a `CamelStoreDBFolderRecord` on its own
/// stack, gives it to the vfunc to fill, writes it, and then clears it with
/// `camel_store_db_folder_record_clear`.
///
/// For this provider, only the plumbing changes. Be careful here, because a
/// port that looks correct can easily be wrong. The `bdata` of the record is
/// still a `g_malloc`ed string that the record owns, and `record_clear`
/// `g_free`s it. Thus [`store_state`] gives the same value as for the older
/// release. Do not allocate a record and return a pointer to it, as the older
/// signature does. On this release, that leaks the record and loses the state.
#[cfg(not(camel_summary_records))]
unsafe extern "C" fn summary_header_save(
    summary: *mut CamelFolderSummary,
    record: *mut CamelSummaryFolderRecord,
    error: *mut *mut GError,
) -> gboolean {
    guard_summary("summary_header_save", summary, GFALSE, || {
        // SAFETY: chaining up first, so that each count and flag Camel keeps
        // in the record comes from the class that owns it, with the arguments
        // Camel passed through unchanged.
        let chained = unsafe { parent_class() }
            .and_then(|class| class.summary_header_save)
            .map_or(GFALSE, |save| unsafe { save(summary, record, error) });
        if chained == GFALSE || record.is_null() {
            return chained;
        }

        // SAFETY: `summary` is of this type, by the guard; the record is live
        // for the call and the parent does not write `bdata`.
        unsafe { store_state(summary, record) };
        GTRUE
    })
}

/// Writes the state into the `bdata` of a record, for both vfuncs above.
///
/// It writes all of the field and does not append, because a header's `bdata`
/// is not shared through the class chain (see the module documentation). It
/// frees the old value of the field first: if a base class starts to use the
/// field, a leak at each save would occur otherwise.
///
/// A summary with no state stores nothing. Thus a mailbox that has never
/// listed loads as a summary with no state, not as an empty-string state.
///
/// # Safety
///
/// `summary` must be a live summary of this type, and `record` a live record
/// whose `bdata` nothing else holds.
unsafe fn store_state(summary: *mut CamelFolderSummary, record: *mut CamelSummaryFolderRecord) {
    // SAFETY: the contract above, and `g_string_free` gives a `g_malloc`ed
    // string that the record takes ownership of. The record frees it on 3.52,
    // and `camel_store_db_folder_record_clear` frees it from 3.58.
    unsafe {
        let Some(state) = summary_state(summary) else {
            return;
        };
        let bdata = g_string_new(ptr::null());
        camel_util_bdata_put_number(bdata, HEADER_VERSION);
        camel_util_bdata_put_string(bdata, c_string(state.as_str()).as_ptr());
        g_free((*record).bdata.cast());
        (*record).bdata = g_string_free(bdata, GFALSE);
    }
}

/// `CamelFolderSummaryClass.summary_header_load`: reads the state from the
/// record that Camel stored.
///
/// A record without data from this class (written before this column existed,
/// or by a different provider for the same folder name) does not change the
/// state, and the function still reports success. A failure would reject all of
/// the header, with the counts and the next uid, because of one field that
/// nothing else needs.
unsafe extern "C" fn summary_header_load(
    summary: *mut CamelFolderSummary,
    record: *mut CamelSummaryFolderRecord,
) -> gboolean {
    guard_summary("summary_header_load", summary, GFALSE, || {
        // SAFETY: chaining up first, on an instance of a type derived from the
        // parent, with the record Camel passed through unchanged.
        let chained = unsafe { parent_class() }
            .and_then(|class| class.summary_header_load)
            .map_or(GFALSE, |load| unsafe { load(summary, record) });
        if chained == GFALSE || record.is_null() {
            return chained;
        }

        // SAFETY: the record is live for the call, and the cursor is a copy of
        // its `bdata` pointer that the reads below move through the string
        // without taking ownership of it; `get_string` gives a `g_malloc`ed
        // copy that this function frees.
        let stored = unsafe {
            let mut cursor = (*record).bdata;
            if cursor.is_null() {
                return GTRUE;
            }
            if camel_util_bdata_get_number(ptr::addr_of_mut!(cursor), 0) != HEADER_VERSION {
                return GTRUE;
            }
            let text = camel_util_bdata_get_string(ptr::addr_of_mut!(cursor), ptr::null());
            if text.is_null() {
                return GTRUE;
            }
            let state = CStr::from_ptr(text).to_string_lossy().into_owned();
            g_free(text.cast());
            state
        };

        // SAFETY: `summary` is of this type, by the guard above.
        unsafe { set_summary_state(summary, State::new(stored)) };
        GTRUE
    })
}

/// The class of the parent, for chaining up.
///
/// Use the *parent's* class, not the class of this type, for the reason that
/// [`crate::message_info`] gives. With the class of this type, a further
/// subclass would chain into these same functions and recurse until the stack
/// overflows.
///
/// # Safety
///
/// An instance of this type must exist. This makes sure that the class of its
/// parent is initialised and alive.
unsafe fn parent_class<'a>() -> Option<&'a CamelFolderSummaryClass> {
    // SAFETY: the contract above.
    unsafe { subclass::parent_class::<CamelFolderSummaryClass>(JmapSummary::parent_type()) }
}

/// Runs one vfunc body. It does not run the body on a summary that is not of
/// this type.
///
/// The type check lets the bodies above use the instance struct without a
/// check of their own. The panic guard is the rule for each `extern "C"`
/// function in this repository.
fn guard_summary<T>(
    what: &str,
    summary: *mut CamelFolderSummary,
    fallback: T,
    body: impl FnOnce() -> T,
) -> T {
    // SAFETY: `summary` is the argument Camel dispatched on, so it is NULL or a
    // live instance.
    if unsafe { JmapSummary::borrow(summary) }.is_none() {
        log_critical(&format!(
            "CamelJmapSummary::{what} called on a summary of another type"
        ));
        return fallback;
    }
    guard(what, fallback, body)
}

/// Registers the summary type, or returns it if it is already registered.
pub fn summary_type() -> GType {
    register_static::<JmapSummary>()
}

/// Gives `folder` the summary that keeps its rows, and loads the stored rows.
///
/// [`crate::folder::new_folder`] calls this once, because the summary is
/// constructed with its folder and a folder must always have one. Otherwise
/// there is a time when Camel can ask a folder with `HAS_SUMMARY_CAPABILITY`
/// for a count that it cannot get. `take_folder_summary` takes the reference
/// and does not add one. Thus the folder owns the summary, and only the
/// finalizer of the folder releases it.
///
/// The construction is the same as in `camel_folder_summary_new` (the folder is
/// a construct-only property), but this function does not call it, because
/// this folder needs the subclass above. A load error is ignored.
///
/// # Safety
///
/// `folder` must point at a live `CamelFolder` that has no summary yet.
pub unsafe fn attach_summary(folder: *mut CamelFolder) {
    // SAFETY: a variadic construct call on a registered type; `folder` is a
    // live `CamelFolder` by this function's contract, which is the type the
    // property carries, and the list is NULL-terminated. The summary is given
    // directly to the folder with its ownership.
    unsafe {
        let summary = g_object_new(
            summary_type(),
            c"folder".as_ptr(),
            folder,
            ptr::null::<gchar>(),
        )
        .cast::<CamelFolderSummary>();
        let mut error: *mut GError = ptr::null_mut();
        camel_folder_summary_load(summary, ptr::addr_of_mut!(error));
        if !error.is_null() {
            glib_sys::g_error_free(error);
        }
        camel_folder_take_folder_summary(folder, summary);
    }
}

/// Makes the summary agree with one listing of the mailbox, and reports the
/// changes.
///
/// All of the work occurs under the lock of the summary, so a reader never
/// sees the folder during a refresh. Half a mailbox is a worse result than the
/// previous one.
///
/// The returned [`Changes`] are the other half of the result. New rows update a
/// folder that is *opened* later. Only the diff updates a folder that is
/// already open. The function returns the diff and does not emit it, because a
/// `CamelFolder` emits and this function gets only a summary. This separation
/// also lets the tests examine each rule without a signal.
///
/// # Safety
///
/// `summary` must point at a live `CamelFolderSummary`.
pub unsafe fn apply_listing(
    summary: *mut CamelFolderSummary,
    messages: &[MessageSummary],
) -> Changes {
    let listed: BTreeSet<&str> = messages
        .iter()
        .map(|message| message.uid.as_str())
        .collect();
    let mut changes = Changes::new();

    // SAFETY: the summary is live by this function's contract, and Camel's
    // summary lock is recursive: the calls below take it again themselves.
    unsafe {
        camel_folder_summary_lock(summary);
        remove_absent(summary, &listed, &mut changes);
        for message in messages {
            apply_message(summary, message, &mut changes);
        }
        camel_folder_summary_unlock(summary);
    }

    changes
}

/// Makes the summary agree with one *delta* of the mailbox, and reports the
/// changes.
///
/// `present` holds the rows of this mailbox for the messages that the delta
/// names. `absent` holds the uids that the mailbox does not hold now. The rows
/// that neither list names do not change. That is the difference from
/// [`apply_listing`], and the reason for a second function.
///
/// The lock is the same, and the result is the same [`Changes`], with one more
/// list that a listing leaves empty. When the delta names a message that has no
/// row, the message arrived after the state of the delta request. Thus it goes
/// on the recent list of Camel and also on the added list.
/// [`crate::changes`] tells why only this path can do that.
///
/// # Safety
///
/// `summary` must point at a live `CamelFolderSummary`.
pub unsafe fn apply_delta(
    summary: *mut CamelFolderSummary,
    present: &[MessageSummary],
    absent: &[Id],
) -> Changes {
    let mut changes = Changes::new();

    // SAFETY: the summary is live by this function's contract, and Camel's
    // summary lock is recursive: the calls below take it again themselves.
    unsafe {
        camel_folder_summary_lock(summary);
        for uid in absent {
            remove_row(summary, uid.as_str(), &mut changes);
        }
        for message in present {
            if apply_message(summary, message, &mut changes) {
                changes.arrive(message.uid.as_str());
            }
        }
        camel_folder_summary_unlock(summary);
    }

    changes
}

/// Removes the row for one uid that a delta reports as absent, if the folder
/// has that row.
///
/// Most uids that a delta calls absent were never in this folder.
/// `Email/changes` reports on all mailboxes of the account, so a message that
/// the user moved to a different folder is also on the absent list of this
/// folder. To report those uids would tell Camel to remove lines that the
/// message list does not have. It would also make a delta about a different
/// folder look like a change here, and that controls whether the folder emits
/// `changed`.
///
/// # Safety
///
/// `summary` must be live and locked.
unsafe fn remove_row(summary: *mut CamelFolderSummary, uid: &str, changes: &mut Changes) {
    let key = c_string(uid);
    // SAFETY: the uid outlives both calls, and neither takes ownership of it.
    unsafe {
        if camel_folder_summary_check_uid(summary, key.as_ptr()) == GFALSE {
            return;
        }
        camel_folder_summary_remove_uid(summary, key.as_ptr());
    }
    changes.remove(uid);
}

/// Removes the rows for the messages that the listing does not name.
///
/// # Safety
///
/// `summary` must be live and locked.
unsafe fn remove_absent(
    summary: *mut CamelFolderSummary,
    listed: &BTreeSet<&str>,
    changes: &mut Changes,
) {
    // SAFETY: `summary_dup_uids` gives a snapshot that the caller owns, with
    // its own reference to each uid in it. Thus the removal of a row during
    // the loop does not free the string being read and does not disturb the
    // loop.
    unsafe {
        let existing = summary_dup_uids(summary);
        if existing.is_null() {
            return;
        }
        for index in 0..(*existing).len {
            let uid: *const gchar = (*existing).pdata.add(index as usize).read().cast();
            if uid.is_null() {
                continue;
            }
            // A stored uid that is not valid UTF-8 cannot be a uid that the
            // listing names, so its row is removed too.
            let text = std::ffi::CStr::from_ptr(uid).to_str();
            if text.is_ok_and(|uid| listed.contains(uid)) {
                continue;
            }
            camel_folder_summary_remove_uid(summary, uid);
            // Report the removal under the name that Camel uses, when there is
            // one. A removal under a name that the message list does not have
            // leaves the line on the screen.
            if let Ok(text) = text {
                changes.remove(text);
            }
        }
        summary_free_uids(existing);
    }
}

/// Adds the row for one message, or updates the row that is already there.
///
/// Returns `true` when the folder did not have the row. [`apply_delta`] uses
/// this to identify an arrival. [`apply_listing`] ignores it, because a listing
/// gives the full mailbox. The return value tells only that the row is new.
/// The function puts the change on `changes` in both cases, when the change is
/// worth a report to Camel.
///
/// # Safety
///
/// `summary` must be live and locked.
unsafe fn apply_message(
    summary: *mut CamelFolderSummary,
    message: &MessageSummary,
    changes: &mut Changes,
) -> bool {
    let uid = c_string(message.uid.as_str());

    // SAFETY: the uid outlives every call it is passed to; `summary_get`
    // returns a reference this function owns and releases when `info` drops
    // at the end of its scope, and `summary_add` takes a reference of its own
    // to the row built here; this function's own reference to that row is
    // released the same way.
    unsafe {
        if camel_folder_summary_check_uid(summary, uid.as_ptr()) != GFALSE
            && let Some(info) = Owned::from_raw(camel_folder_summary_get(summary, uid.as_ptr()))
        {
            // Report the row only when it changed. A refresh is a poll, so
            // almost all rows that it gets are unchanged. If the folder reported
            // all of them, the message list would redraw at each poll.
            if update_message_info(info.as_ptr(), message) {
                changes.change(message.uid.as_str());
            }
            return false;
        }
        // Execution continues here for a uid that the summary does not know
        // (the usual case of a new message). It also continues here for a uid
        // that the summary knows but has no row for, because the database of
        // the summary is missing. For that second case, the recovery is to
        // build the row again from the listing, which the code below does.

        let Some(info) = Owned::from_raw(new_message_info(message)) else {
            return false;
        };
        // `force_keep_uid` is TRUE because the uid is the JMAP `Email` id. It is
        // unique in the mailbox and immutable, and each later request uses it.
        // A row with a uid from the counter of the summary cannot be fetched.
        // The flag states this but does not enforce it: Camel changes only a
        // uid that is empty or already loaded, and the check above stops the
        // second case. The value stays correct if that case ever occurs.
        camel_folder_summary_add(summary, info.as_ptr(), GTRUE);
        // Clear the pending-write mark that `add` sets. Camel marks an added
        // row as data to send to the server. That is correct for a message that
        // the user composed and appended. Here it is incorrect, because the row
        // exists *because* the server described it. If the mark stays,
        // [`crate::synchronize`] writes each message of each mailbox back to
        // the server that just listed it.
        clear_pending_write(info.as_ptr());
        changes.add(message.uid.as_str());
        true
    }
}
