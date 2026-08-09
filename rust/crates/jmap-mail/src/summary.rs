// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelFolderSummary`: where a folder keeps the rows [`crate::message_info`]
//! builds.
//!
//! A `CamelFolder` on its own is a name and a store. The summary is its
//! contents: Camel asks it for the message count shown beside the folder, for
//! the uid list the message list is drawn from, and for the row behind each
//! line of it. That is why a folder without one is a folder Camel never asks
//! anything of — `CAMEL_FOLDER_HAS_SUMMARY_CAPABILITY` is the flag it tests
//! first, and [`crate::folder`] could not honestly set it until now.
//!
//! ## Two lines of subclass
//!
//! `CamelJmapSummary` overrides two vfuncs. What a `CamelFolderSummary` subclass
//! usually exists for is building rows out of *messages*: the three
//! `message_info_new_from_*` vfuncs, which turn a parser, a MIME message or a
//! header list into a row, and `next_uid_string`, which invents a uid for a
//! message that arrived without one. A JMAP folder is listed rather than
//! parsed — the rows come from `Email/get`, already structured, and every one
//! of them arrives with the server's own immutable id — so all four would be
//! overrides of paths this provider does not take.
//!
//! It exists first for the one thing a summary decides that is not a vfunc at
//! all: `message_info_type`, the class it instantiates a row as. Camel reads
//! that field when it loads the summary back out of the database, so a folder
//! whose summary declared nothing would come back from a restart holding plain
//! `CamelMessageInfoBase` rows — and with them no [`server keywords`], which is
//! the column that makes a flag change a difference rather than a guess. The
//! rows [`crate::message_info`] builds are of that type either way; this is what
//! makes the ones Camel builds match them.
//!
//! [`server keywords`]: crate::message_info::server_keywords
//!
//! ## The state the rows are current as of
//!
//! The two vfuncs it does override are `summary_header_save` and
//! `summary_header_load`, and what they carry is one string: the `Email` state
//! the last listing of this mailbox was taken at. It lives on the *summary*
//! rather than on the folder because it is a fact about the rows — it says what
//! they are current as of — so it has to be stored where they are stored and
//! read back when they are read back. A state kept only in memory would be no
//! state at all after a restart, which is the case that matters most: the first
//! refresh of every session is exactly the one that would otherwise list every
//! mailbox in full.
//!
//! Camel keeps one header record per folder beside the rows, and reserves a
//! `bdata` field in it for whatever the provider has that Camel has none of —
//! the same arrangement as `CamelMIRecord.bdata`, which [`crate::message_info`]
//! keeps the keywords in, with one difference that decides the format below: a
//! row's `bdata` is written and read through a cursor the whole class chain
//! shares, and a header's is not — `summary_header_load` is handed the record
//! and nothing else. So the field belongs to the last class in the chain, and
//! this one writes it whole rather than appending to it.
//!
//! It carries a format number before the state for the case that is not a
//! restart but an upgrade: a header written by some later version of this
//! provider is one this version must not read as a state, and a number it does
//! not recognise leaves the summary with no state at all. That costs one full
//! listing, which is what a folder does today anyway; misreading the field
//! would cost the mailbox.
//!
//! ## A listing is not a copy
//!
//! [`apply_listing`] is called again every time the folder is refreshed, so
//! most of what it does is meet rows that are already there. Three rules, and
//! they are what the tests are about:
//!
//! - A message the listing no longer names has left the mailbox — JMAP moves
//!   mail by changing `mailboxIds`, so leaving and being deleted look the same
//!   from inside one folder — and its row goes.
//! - A message that is already there keeps its row and has only its mutable
//!   columns rewritten. RFC 8621 §4.1 makes everything but `keywords` and
//!   `mailboxIds` immutable, so there is nothing else a refresh could have
//!   learnt, and replacing the row would throw away the local marks Camel
//!   keeps in the same flags word.
//! - A message that is new gets a row under the server's id, not under one the
//!   summary invented.

use std::collections::BTreeSet;
use std::ffi::CStr;
use std::ptr;
use std::sync::Mutex;

use eds_sys::{
    CamelFIRecord, CamelFolder, CamelFolderSummary, CamelFolderSummaryClass,
    camel_folder_summary_add, camel_folder_summary_check_uid, camel_folder_summary_free_array,
    camel_folder_summary_get, camel_folder_summary_get_array, camel_folder_summary_get_type,
    camel_folder_summary_lock, camel_folder_summary_remove_uid, camel_folder_summary_touch,
    camel_folder_summary_unlock, camel_folder_take_folder_summary, camel_util_bdata_get_number,
    camel_util_bdata_get_string, camel_util_bdata_put_number, camel_util_bdata_put_string,
};
use glib_sys::{
    GError, GFALSE, GTRUE, GType, g_free, g_string_free, g_string_new, gboolean, gchar,
};
use gobject_sys::{g_object_new, g_object_unref, g_type_check_instance_is_a, g_type_class_peek};
use jmap_backend_core::instance::Slot;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use jmap_backend_core::trampoline::{guard, log_critical};
use jmap_mail_sync::MessageSummary;
use jmap_proto::State;

use crate::changes::Changes;
use crate::folder_info::c_string;
use crate::message_info::{
    clear_pending_write, message_info_type, new_message_info, update_message_info,
};

/// What the stored header says it is. Bumped when the fields after it change
/// meaning; a header carrying anything else is read as a summary with no state,
/// which is the same position a folder that has never listed is in.
const HEADER_VERSION: i64 = 1;

/// The instance struct: Camel's summary, and the one thing about the rows in it
/// that Camel has no field for.
#[repr(C)]
pub struct JmapSummary {
    parent: CamelFolderSummary,
    /// The `Email` state the listing these rows came from was taken at, or
    /// `None` for a summary that has never listed — which is what makes the
    /// next refresh list rather than ask for a delta from a state it invented.
    ///
    /// A [`Slot`] because the struct arrives zeroed and is freed without a
    /// destructor running over it, and a [`Mutex`] because it is written every
    /// refresh and Camel drives a folder from more than one thread.
    state: Slot<Mutex<Option<State>>>,
}

impl JmapSummary {
    /// The Rust view of a summary Camel handed over, or `None` for one that is
    /// not ours.
    ///
    /// Checked rather than assumed for the reason [`crate::message_info`]
    /// checks a row: a summary reaches this crate from a folder, and a folder
    /// built before this type existed carries a plain `CamelFolderSummary`.
    ///
    /// # Safety
    ///
    /// `summary` must be NULL or point at a live `CamelFolderSummary`.
    unsafe fn borrow<'a>(summary: *mut CamelFolderSummary) -> Option<&'a Self> {
        // SAFETY: the type check is what makes the cast sound; the contract
        // above is what makes the check itself legal.
        unsafe {
            if summary.is_null()
                || g_type_check_instance_is_a(summary.cast(), summary_type()) == GFALSE
            {
                return None;
            }
            summary.cast::<Self>().as_ref()
        }
    }
}

/// The class struct, and the field this type exists for.
#[repr(C)]
pub struct JmapSummaryClass {
    parent_class: CamelFolderSummaryClass,
}

// SAFETY: both structs are #[repr(C)] and lead with the CamelFolderSummary
// instance and class structs, whose layouts eds-sys's tests/layout.rs checks
// against `g_type_query`; CamelFolderSummary derives from GObject.
unsafe impl ObjectSubclass for JmapSummary {
    const NAME: &'static CStr = c"CamelJmapSummary";
    type Instance = JmapSummary;
    type Class = JmapSummaryClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { camel_folder_summary_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: the class leads with CamelFolderSummaryClass — the contract
        // above.
        let class = unsafe { &mut (*class).parent_class };
        // Camel instantiates a row of this type whenever it builds one itself —
        // which is every row of every folder after a restart, read back out of
        // the summary database.
        class.message_info_type = message_info_type();
        class.summary_header_save = Some(summary_header_save);
        class.summary_header_load = Some(summary_header_load);
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // Filled here rather than left empty, so that an unlisted summary
        // answers `None` because it holds an empty state and not because its
        // slot was never filled — the second is a bug that would read as the
        // first.
        //
        // SAFETY: the instance is being constructed, so this is the only
        // reference to it.
        unsafe { (*instance).state.init(Mutex::new(None)) };
    }

    unsafe fn finalize(instance: *mut Self::Instance) {
        // SAFETY: the instance is being finalized, so nothing can still reach
        // it and no borrow handed out by `get` is alive.
        unsafe { (*instance).state.clear() };
    }
}

/// The `Email` state `summary`'s rows are current as of, or `None` for a
/// summary that has never listed — or one that is not this provider's.
///
/// A copy rather than a borrow, like the row's keywords and for the same
/// reason: the value lives behind a mutex the summary hands nothing out of, so
/// a refresh renewing it cannot be interleaved with a reader.
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

/// Records the state one listing was taken at, replacing whatever the summary
/// remembered.
///
/// Touches the summary, because a state that is not written back is a state
/// that does not survive the folder: Camel skips saving a summary it was not
/// told had changed, and a refresh that found no new mail changes nothing else.
///
/// Silently does nothing for a summary that is not ours, which is the same
/// degradation `None` is — a folder that cannot remember a state is one that
/// lists.
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
            // The usual answer for a mailbox nobody wrote to. Marking the
            // summary dirty for it would have Camel rewrite the database on
            // every poll of every folder.
            return;
        }
        *slot = Some(state);
    }
    // SAFETY: the summary is live and of this type, by the borrow above.
    unsafe { camel_folder_summary_touch(summary) };
}

/// `CamelFolderSummaryClass.summary_header_save`: puts the state into the
/// record Camel is about to store.
///
/// The record is the parent's — chained up for rather than allocated here, so
/// that every count and flag Camel keeps in it is filled by the class that owns
/// them. Only `bdata` is ours, and it is written whole for the reason this
/// module's header gives: a header's `bdata` is not a chain the way a row's is.
/// Whatever was in the field is freed first anyway, because a base class that
/// started using it would otherwise leak it once per save.
///
/// A summary with no state stores nothing, so a mailbox that has never been
/// listed reads back as one rather than as a state that is the empty string.
unsafe extern "C" fn summary_header_save(
    summary: *mut CamelFolderSummary,
    error: *mut *mut GError,
) -> *mut CamelFIRecord {
    guard_summary("summary_header_save", summary, ptr::null_mut(), || {
        // SAFETY: chaining up to the parent's own save, on an instance of a
        // type derived from it, with the arguments Camel passed through
        // untouched.
        let record = unsafe { parent_class() }
            .and_then(|class| class.summary_header_save)
            .map_or(ptr::null_mut(), |save| unsafe { save(summary, error) });
        if record.is_null() {
            return record;
        }

        // SAFETY: `summary` is one of ours, by the guard; the record is the one
        // the parent just allocated, so nothing else holds its `bdata`, and
        // `g_string_free` hands over a `g_malloc`ed string Camel frees with the
        // record.
        unsafe {
            let Some(state) = summary_state(summary) else {
                return record;
            };
            let bdata = g_string_new(ptr::null());
            camel_util_bdata_put_number(bdata, HEADER_VERSION);
            camel_util_bdata_put_string(bdata, c_string(state.as_str()).as_ptr());
            g_free((*record).bdata.cast());
            (*record).bdata = g_string_free(bdata, GFALSE);
        }
        record
    })
}

/// `CamelFolderSummaryClass.summary_header_load`: reads it back out of the
/// record Camel stored.
///
/// A record with nothing of ours in it — a summary written before this column
/// existed, or by another provider under the same folder name — leaves the
/// state as it was and still reports success. Failing instead would refuse the
/// whole header, and with it the counts and the next uid, over the one field
/// nothing else needs.
unsafe extern "C" fn summary_header_load(
    summary: *mut CamelFolderSummary,
    record: *mut CamelFIRecord,
) -> gboolean {
    guard_summary("summary_header_load", summary, GFALSE, || {
        // SAFETY: chaining up first, on an instance of a type derived from the
        // parent, with the record Camel passed through untouched.
        let chained = unsafe { parent_class() }
            .and_then(|class| class.summary_header_load)
            .map_or(GFALSE, |load| unsafe { load(summary, record) });
        if chained == GFALSE || record.is_null() {
            return chained;
        }

        // SAFETY: the record is live for the call, and the cursor is a copy of
        // its `bdata` pointer that the reads below advance through the string
        // without taking ownership of it; `get_string` hands back a `g_malloc`ed
        // copy this function frees.
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

        // SAFETY: `summary` is one of ours, by the guard above.
        unsafe { set_summary_state(summary, State::new(stored)) };
        GTRUE
    })
}

/// The parent's class, for chaining up.
///
/// The *parent's* rather than this one's, for the reason
/// [`crate::message_info`] gives: peeking our own would make a further subclass
/// chain into these same functions and recurse until the stack ran out.
///
/// # Safety
///
/// An instance of this type must exist, which is what guarantees its parent's
/// class is initialised and alive.
unsafe fn parent_class<'a>() -> Option<&'a CamelFolderSummaryClass> {
    // SAFETY: the contract above; the class is owned by the type system and
    // outlives every instance.
    unsafe {
        g_type_class_peek(JmapSummary::parent_type())
            .cast::<CamelFolderSummaryClass>()
            .as_ref()
    }
}

/// Runs one vfunc body, refusing to run it at all on a summary that is not
/// ours.
///
/// The type check is what lets the bodies above reach the instance struct
/// without one of their own; the panic guard is the rule every `extern "C"` in
/// this repository follows.
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

/// Gives `folder` the summary it keeps its rows in.
///
/// Called once, from [`crate::folder::new_folder`], because a summary is
/// constructed with the folder it belongs to and a folder is never without
/// one: the alternative is a window in which Camel could ask a folder that
/// claims `HAS_SUMMARY_CAPABILITY` for a count it has nowhere to get.
/// `take_folder_summary` takes the reference rather than adding to it, which
/// is what makes the folder the summary's owner and its finalizer the only
/// place the summary is released.
///
/// Constructed the way `camel_folder_summary_new` constructs a base one — the
/// folder is a construct-only property — rather than through it, because what
/// this folder needs is the subclass above.
///
/// # Safety
///
/// `folder` must point at a live `CamelFolder` that has not been given a
/// summary yet.
pub unsafe fn attach_summary(folder: *mut CamelFolder) {
    // SAFETY: a variadic construct call on a registered type; `folder` is a
    // live `CamelFolder` by this function's contract, which is the type the
    // property carries, and the list is NULL-terminated. The summary is handed
    // straight over along with its ownership.
    unsafe {
        let summary = g_object_new(
            summary_type(),
            c"folder".as_ptr(),
            folder,
            ptr::null::<gchar>(),
        )
        .cast::<CamelFolderSummary>();
        camel_folder_take_folder_summary(folder, summary);
    }
}

/// Brings the summary in line with what one listing of the mailbox found, and
/// reports what moved.
///
/// The whole reconciliation happens under the summary's lock, so a reader
/// never sees the folder mid-refresh — half a mailbox is a worse answer than
/// the previous one.
///
/// The [`Changes`] that come back are the other half of the answer: rewriting
/// the rows brings a folder that is *opened* up to date, and the diff is the
/// only thing that brings one that is already open up to date. It is returned
/// rather than emitted here because emitting is a fact about a `CamelFolder`
/// and this function is given a summary — the same separation the rest of the
/// module keeps, and what lets every rule below be tested without a signal to
/// listen for.
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
    // summary lock is recursive — the calls below take it again themselves.
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

/// Drops the rows for messages the listing did not name.
///
/// # Safety
///
/// `summary` must be live and locked.
unsafe fn remove_absent(
    summary: *mut CamelFolderSummary,
    listed: &BTreeSet<&str>,
    changes: &mut Changes,
) {
    // SAFETY: `get_array` hands back a snapshot the caller owns, holding a
    // reference of its own to every uid in it — so removing a row while
    // walking it neither frees the string being read nor disturbs the walk.
    unsafe {
        let existing = camel_folder_summary_get_array(summary);
        if existing.is_null() {
            return;
        }
        for index in 0..(*existing).len {
            let uid: *const gchar = (*existing).pdata.add(index as usize).read().cast();
            if uid.is_null() {
                continue;
            }
            // A uid Camel stored and we cannot read back as text is not one
            // this listing can have named either, so it goes with the rest.
            let text = std::ffi::CStr::from_ptr(uid).to_str();
            if text.is_ok_and(|uid| listed.contains(uid)) {
                continue;
            }
            camel_folder_summary_remove_uid(summary, uid);
            // Reported under the name Camel keeps it by, when there is one: a
            // removal announced under a name the message list never held is a
            // line that stays on screen.
            if let Ok(text) = text {
                changes.remove(text);
            }
        }
        camel_folder_summary_free_array(existing);
    }
}

/// Adds the row for one message, or updates the one that is already there.
///
/// # Safety
///
/// `summary` must be live and locked.
unsafe fn apply_message(
    summary: *mut CamelFolderSummary,
    message: &MessageSummary,
    changes: &mut Changes,
) {
    let uid = c_string(message.uid.as_str());

    // SAFETY: the uid outlives every call it is passed to; `summary_get`
    // returns a reference this function owns and drops, and `summary_add`
    // takes a reference of its own to the row built here.
    unsafe {
        if camel_folder_summary_check_uid(summary, uid.as_ptr()) != GFALSE {
            let info = camel_folder_summary_get(summary, uid.as_ptr());
            if !info.is_null() {
                // Reported only when the row really moved. A refresh is a poll,
                // so nearly every row it meets is the row it left there, and a
                // folder that announced all of them would redraw the message
                // list the user is reading every time the timer went off.
                if update_message_info(info, message) {
                    changes.change(message.uid.as_str());
                }
                g_object_unref(info.cast());
                return;
            }
            // A uid the summary lists and has no row for is a summary whose
            // database went missing under it. Rebuilding the row from the
            // listing is the recovery; falling through does exactly that.
        }

        let info = new_message_info(message);
        if info.is_null() {
            return;
        }
        // `force_keep_uid`, because the uid is the JMAP `Email` id: unique in
        // the mailbox, immutable, and what every later request names the
        // message by, so a row numbered from the summary's own counter is one
        // nothing can be fetched for. It says that rather than prevents it:
        // Camel renumbers only a uid that is empty or already loaded, and the
        // second cannot get this far past the check above. The value is the
        // one that stays right if it ever does.
        camel_folder_summary_add(summary, info, GTRUE);
        // And straight back off the work list `add` just put it on. Camel marks
        // an added row as having to reach the server, which is right for the
        // caller that function was written for — a message the user composed and
        // appended — and backwards here: this row exists *because* the server
        // described it. Left set, the bit would have
        // [`crate::synchronize`] write every message of every mailbox back to
        // the server it was just listed from.
        clear_pending_write(info);
        g_object_unref(info.cast());
        changes.add(message.uid.as_str());
    }
}
