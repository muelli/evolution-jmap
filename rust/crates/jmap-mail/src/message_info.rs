// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelJmapMessageInfo`: one summary row, as the object Camel keeps it in.
//!
//! `jmap-mail-sync`'s [`MessageSummary`] is the same row read off an
//! `Email/get`, and most of what happens here is copying it across. Three
//! columns are not a copy, and they are what this module is for:
//!
//! - **The flags word.** Camel keeps a message's state as bits in one `guint32`
//!   where JMAP keeps it as a set of keywords. The sync layer has already
//!   sorted the keywords into the ones Camel has a bit for and the ones it does
//!   not; turning the first into the word is `flags_word`, and the second
//!   become Camel's user flags, which is what Evolution shows as labels.
//! - **The address columns.** JMAP sends `from`, `to` and `cc` as structured
//!   name/address pairs; Camel's summary stores one display string per header.
//!   Formatting that string is `CamelInternetAddress`'s job — RFC 5322's
//!   quoting rules live there — so `address_list` builds one and asks it.
//! - **The threading columns.** Camel does not store a `Message-ID`; it stores
//!   64 bits of an MD5 over it, per `CamelSummaryMessageID`, and an array of
//!   the same for the ancestors. There is no public function to compute one,
//!   so `message_id_digest` computes it — and it computes exactly what
//!   Camel's own `camel_message_info_new_from_headers` would have, because
//!   those two paths meet: a message Camel parses locally lands in the same
//!   summary as a row built here, and digests that disagreed would thread one
//!   conversation as two. `tests/message_info.rs` pins that against Camel
//!   itself rather than against a constant.
//!
//! There is a fourth column, and it is neither a copy nor a computation: it is
//! something only this provider knows, so Camel has no field for it and the row
//! has to be a subclass to carry it.
//!
//! ## The keywords the server was last seen holding
//!
//! A flag change is a *difference* on the wire — `jmap-mail-sync`'s
//! `KeywordChange` — because a whole-set write would speak for every keyword on
//! the message, including the ones no client here has heard of. A difference
//! needs both ends, and the row only has one of them: the user marks a message
//! read by mutating the flags word in place, which is the *after*. The before —
//! the keywords the last listing found — is gone the moment that mark is made,
//! unless the row keeps it.
//!
//! So [`JmapMessageInfo`] is a `CamelMessageInfoBase` with the listing's keyword
//! set beside it, written by [`new_message_info`] and renewed by
//! [`update_message_info`]. It is the shape IMAPX's own message info takes for
//! the same reason — it keeps a `server_flags` word next to Camel's — and it
//! survives a restart the only way Camel offers a provider: the summary
//! database row's `bdata`, one string per row that the class chain appends to on
//! the way out and reads back in the same order on the way in. (The row struct
//! is `CamelMIRecord` up to EDS 3.52 and `CamelStoreDBMessageRecord` from 3.58;
//! nothing but its name changed, so the two vfuncs below name it through
//! [`eds_sys::compat::CamelSummaryMessageRecord`] and are otherwise the same
//! functions on both.)
//!
//! Two things follow from what the column is *for*, and both are tested:
//!
//! - **A row that lost it remembers nothing rather than failing to load.** The
//!   empty set is the conservative answer and not merely a tolerable one: a
//!   difference from nothing only ever adds keywords, so a row whose stored data
//!   predates this column takes none off.
//! - **A clone into a summary of ours carries it.** A copy that dropped the
//!   column would be a row whose next flag change looked like the removal of
//!   every keyword the message has. Which summary the copy is being made for is
//!   what decides whether it is a row of ours at all — see `clone`.
//!
//! The row is still built detached, with no summary behind it — nothing about
//! the column needs one — but which class a *summary* instantiates is no longer
//! a question that answers itself. [`crate::summary`]'s subclass declares this
//! type, which is what a row read back out of the database comes back as, and
//! `tests/summary.rs` pins the two answers together.

use std::ffi::CStr;
use std::ptr;
use std::sync::Mutex;

use eds_sys::compat::CamelSummaryMessageRecord;
use eds_sys::{
    CAMEL_MESSAGE_ANSWERED, CAMEL_MESSAGE_ATTACHMENTS, CAMEL_MESSAGE_DRAFT, CAMEL_MESSAGE_FLAGGED,
    CAMEL_MESSAGE_FORWARDED, CAMEL_MESSAGE_JUNK, CAMEL_MESSAGE_NOTJUNK, CAMEL_MESSAGE_SEEN,
    CamelAddress, CamelFolderSummary, CamelMessageFlags, CamelMessageInfo, CamelMessageInfoBase,
    CamelMessageInfoBaseClass, CamelMessageInfoClass, camel_address_format,
    camel_internet_address_add, camel_internet_address_new, camel_message_info_base_get_type,
    camel_message_info_dup_user_flags, camel_message_info_freeze_notifications,
    camel_message_info_get_flags, camel_message_info_get_folder_flagged, camel_message_info_set_cc,
    camel_message_info_set_date_received, camel_message_info_set_date_sent,
    camel_message_info_set_flags, camel_message_info_set_folder_flagged,
    camel_message_info_set_from, camel_message_info_set_message_id, camel_message_info_set_preview,
    camel_message_info_set_size, camel_message_info_set_subject, camel_message_info_set_to,
    camel_message_info_set_uid, camel_message_info_take_references,
    camel_message_info_take_user_flags, camel_message_info_thaw_notifications,
    camel_named_flags_free, camel_named_flags_get, camel_named_flags_get_length,
    camel_named_flags_insert, camel_named_flags_new, camel_util_bdata_get_number,
    camel_util_bdata_get_string, camel_util_bdata_put_number, camel_util_bdata_put_string,
};
use glib_sys::{
    G_CHECKSUM_MD5, GArray, GFALSE, GString, GTRUE, GType, g_array_append_vals, g_array_sized_new,
    g_checksum_free, g_checksum_get_digest, g_checksum_new, g_checksum_update, g_free, gboolean,
    gchar,
};
use gobject_sys::g_object_new;
use jmap_backend_core::instance::Slot;
use jmap_backend_core::marshal::checked_borrow;
use jmap_backend_core::owned::Owned;
use jmap_backend_core::subclass::{self, ObjectSubclass, register_static};
use jmap_backend_core::trampoline::{guard, log_critical};
use jmap_mail_sync::{KeywordChange, Keywords, MessageFlags, MessageSummary};
use jmap_proto::mail::EmailAddress;

use crate::folder_info::c_string;

/// The instance struct: Camel's row, and the one thing about it Camel has no
/// field for.
#[repr(C)]
pub struct JmapMessageInfo {
    parent: CamelMessageInfoBase,
    /// The keywords the last listing found on the message.
    ///
    /// A [`Slot`] for the reason every other instance field in this crate is
    /// one — the struct arrives zeroed and is freed without a destructor
    /// running over it — and a [`Mutex`] because, unlike a folder's mailbox id,
    /// this one is written more than once: every refresh renews it, and Camel
    /// drives a folder from more than one thread.
    server: Slot<Mutex<Keywords>>,
}

impl JmapMessageInfo {
    /// The Rust view of a row Camel handed over, or `None` for a row that is
    /// not one of ours.
    ///
    /// Checked rather than assumed, unlike a vfunc's own first argument: a row
    /// reaches this crate from a summary, and a summary that was written before
    /// this type existed hands back a plain `CamelMessageInfoBase`. Reading one
    /// of those as this type would be undefined behaviour rather than a wrong
    /// answer.
    ///
    /// # Safety
    ///
    /// `info` must be NULL or point at a live `CamelMessageInfo`.
    unsafe fn borrow<'a>(info: *mut CamelMessageInfo) -> Option<&'a Self> {
        // SAFETY: the contract above is what makes the type check itself
        // legal, and the check is what makes the cast inside sound.
        unsafe { checked_borrow(info, message_info_type()) }
    }
}

/// The class struct, carrying the parent's vfunc slots with our functions in
/// three of them.
#[repr(C)]
pub struct JmapMessageInfoClass {
    parent_class: CamelMessageInfoBaseClass,
}

// SAFETY: both structs are #[repr(C)] and lead with the CamelMessageInfoBase
// instance and class structs, whose layouts eds-sys's tests/layout.rs checks
// against `g_type_query`; CamelMessageInfoBase derives from CamelMessageInfo,
// from GObject.
unsafe impl ObjectSubclass for JmapMessageInfo {
    /// `CamelJmapMessageInfo`, matching the store and the folder: Camel's own
    /// providers name theirs `Camel<Protocol>MessageInfo`.
    const NAME: &'static CStr = c"CamelJmapMessageInfo";
    type Instance = JmapMessageInfo;
    type Class = JmapMessageInfoClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { camel_message_info_base_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: the class leads with CamelMessageInfoBaseClass, which leads
        // with CamelMessageInfoClass — the contract above.
        let class = unsafe { &mut (*class).parent_class.parent_class };
        class.load = Some(load);
        class.save = Some(save);
        class.clone = Some(clone);
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // Filled here rather than left empty, because an empty slot and a slot
        // holding an empty set are different answers: the first is a row this
        // crate cannot speak for, and every reader treats it as one.
        //
        // SAFETY: the instance is being constructed, so this is the only
        // reference to it.
        unsafe { (*instance).server.init(Mutex::new(Keywords::default())) };
    }

    unsafe fn finalize(instance: *mut Self::Instance) {
        // SAFETY: the instance is being finalized, so nothing can still reach
        // it and no borrow handed out by `get` is alive. Without this the set
        // leaks — once per row the folder ever listed.
        unsafe { (*instance).server.clear() };
    }
}

/// Registers the row type, or returns it if it is already registered.
///
/// Statically, like the store's and the folder's: a Camel provider is not a
/// `GTypeModule`, so there is no unload for a dynamic type to be unregistered
/// by.
pub fn message_info_type() -> GType {
    register_static::<JmapMessageInfo>()
}

/// The keywords `info` last saw the server holding, or `None` if it is not a row
/// this provider built.
///
/// A copy rather than a borrow: the set lives behind a mutex the row hands out
/// nothing from, so that a refresh renewing it cannot be interleaved with a
/// synchronisation reading it.
///
/// # Safety
///
/// `info` must be NULL or point at a live `CamelMessageInfo`.
pub unsafe fn server_keywords(info: *mut CamelMessageInfo) -> Option<Keywords> {
    // SAFETY: the contract above, and `borrow` checks the type.
    let row = unsafe { JmapMessageInfo::borrow(info) }?;
    let keywords = row
        .server
        .get()?
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Some(keywords.clone())
}

/// The keyword set the row claims *now* — the other end of a flag change.
///
/// [`server_keywords`] is the before and this is the after, and the asymmetry
/// between them is the point: the before is a column only this provider keeps,
/// while the after is read straight back out of Camel's own two — the flags word
/// and the user flags — because those are what the user's click changed. It is
/// `flags_word` and `set_user_flags` run backwards, over the same bits and
/// the same names, so that a row nobody touched produces the set it was built
/// from and therefore no change at all.
///
/// `attachments` is read out of the word for symmetry and contributes nothing:
/// [`Keywords::new`] leaves it out, because `hasAttachment` is a property RFC
/// 8621 §4.1.1 has the server compute rather than a label a client may set.
///
/// # Safety
///
/// `info` must point at a live `CamelMessageInfo`.
pub unsafe fn row_keywords(info: *mut CamelMessageInfo) -> Keywords {
    // SAFETY: the contract above; `dup_user_flags` hands over a copy this
    // function owns and frees, which is what makes reading it here safe without
    // holding the row's property lock across the walk.
    unsafe {
        let flags = message_flags(camel_message_info_get_flags(info));
        let user_flags = camel_message_info_dup_user_flags(info);
        let tags = (0..camel_named_flags_get_length(user_flags))
            .filter_map(|index| {
                let name = camel_named_flags_get(user_flags, index);
                (!name.is_null()).then(|| CStr::from_ptr(name).to_string_lossy().into_owned())
            })
            .collect::<Vec<String>>();
        camel_named_flags_free(user_flags);
        Keywords::new(&flags, &tags)
    }
}

/// Records what one listing found, replacing whatever the row remembered.
///
/// Wholesale, for the reason the user flags are replaced wholesale: a
/// `keywords` object is the whole truth about the message's keywords, so a
/// keyword the listing did not mention is one that is no longer there.
///
/// Silently does nothing for a row that is not ours — a row loaded from a
/// summary written before this column existed — which is the same degradation
/// an empty set is: the difference from nothing adds keywords and removes none.
///
/// # Safety
///
/// `info` must be NULL or point at a live `CamelMessageInfo`.
pub(crate) unsafe fn set_server_keywords(info: *mut CamelMessageInfo, keywords: Keywords) {
    // SAFETY: the contract above, and `borrow` checks the type.
    let Some(row) = (unsafe { JmapMessageInfo::borrow(info) }) else {
        return;
    };
    if let Some(slot) = row.server.get() {
        *slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = keywords;
    }
}

/// `CamelMessageInfoClass.save`: appends the column to the row's stored data.
///
/// The count first, then the names, because that is what [`load`] reads and the
/// two are one format. Written through `camel_util_bdata_put_string` rather than
/// joined here: `bdata` is one string for the whole class chain, and a keyword
/// may contain a space, a digit or a dash — Camel's own length-prefixed encoding
/// is what keeps one keyword from coming back as two.
///
/// # Safety
///
/// Called by Camel with a live row of this type, a record to fill and the
/// string the chain is building.
unsafe extern "C" fn save(
    info: *const CamelMessageInfo,
    record: *mut CamelSummaryMessageRecord,
    bdata: *mut GString,
) -> gboolean {
    guard_row("save", info.cast_mut(), GFALSE, || {
        // SAFETY: chaining up to the parent's own save, on an instance of a type
        // derived from it, with the arguments Camel passed through untouched.
        let chained = unsafe { parent_class() }
            .and_then(|class| class.save)
            .map_or(GFALSE, |save| unsafe { save(info, record, bdata) });
        if chained == GFALSE {
            return GFALSE;
        }

        // SAFETY: `info` is a row of this type, so the set is there; `bdata` is
        // the live GString Camel is building and each name is NUL-terminated
        // and copied by the call.
        unsafe {
            let keywords = server_keywords(info.cast_mut()).unwrap_or_default();
            camel_util_bdata_put_number(bdata, keywords.len() as i64);
            for name in keywords.iter() {
                camel_util_bdata_put_string(bdata, c_string(name).as_ptr());
            }
        }
        GTRUE
    })
}

/// `CamelMessageInfoClass.load`: reads the column back out of it.
///
/// A count that runs past the end of the string is a truncated or foreign
/// record rather than an error to report: the reads stop at the first name that
/// is not there, and what the row is left with is the keywords that *were*
/// stored. Reporting failure instead would drop the whole row — every other
/// column included — over the one column nothing else needs.
///
/// # Safety
///
/// Called by Camel with a live row of this type, the record it was stored as,
/// and the cursor the chain is reading through.
unsafe extern "C" fn load(
    info: *mut CamelMessageInfo,
    record: *const CamelSummaryMessageRecord,
    cursor: *mut *mut gchar,
) -> gboolean {
    guard_row("load", info, GFALSE, || {
        // SAFETY: chaining up first, because the cursor is read in the order it
        // was written and the parent's fields come first.
        let chained = unsafe { parent_class() }
            .and_then(|class| class.load)
            .map_or(GFALSE, |load| unsafe { load(info, record, cursor) });
        if chained == GFALSE {
            return GFALSE;
        }

        // SAFETY: the cursor is Camel's own, pointing into the record's `bdata`;
        // `get_string` advances it and hands back a `g_malloc`ed string, or NULL
        // once there is nothing left to read.
        let names: Vec<String> = unsafe {
            let count = camel_util_bdata_get_number(cursor, 0).max(0) as u64;
            (0..count)
                .map_while(|_| {
                    let name = camel_util_bdata_get_string(cursor, ptr::null());
                    if name.is_null() {
                        return None;
                    }
                    let text = CStr::from_ptr(name).to_string_lossy().into_owned();
                    g_free(name.cast());
                    Some(text)
                })
                .collect()
        };

        // SAFETY: `info` is a row of this type, by the guard above.
        unsafe { set_server_keywords(info, names.into_iter().collect()) };
        GTRUE
    })
}

/// `CamelMessageInfoClass.clone`: hands the column to the copy.
///
/// Which copies those are is not this function's choice. The parent's clone
/// builds its result out of the *summary* it is told to assign the copy to — it
/// is `camel_message_info_new` on that summary — so a row cloned into a summary
/// of ours comes back as one of ours and a row cloned into no summary at all
/// comes back a plain `CamelMessageInfoBase`. The second is left exactly as the
/// parent made it, which is why the write below goes through the checked
/// accessor rather than through the instance struct: a copy that is not of this
/// type is a copy in a folder that has no JMAP keywords to be asked about.
///
/// Overriding the whole thing to force the type would mean rebuilding every
/// column of the row here, which is the parent's job and would silently stop
/// copying whatever column Camel adds next.
///
/// # Safety
///
/// Called by Camel with a live row of this type.
unsafe extern "C" fn clone(
    info: *const CamelMessageInfo,
    summary: *mut CamelFolderSummary,
) -> *mut CamelMessageInfo {
    guard_row("clone", info.cast_mut(), ptr::null_mut(), || {
        // SAFETY: chaining up to the parent's clone with Camel's own arguments;
        // the row it returns is owned by the caller.
        let copy = unsafe { parent_class() }
            .and_then(|class| class.clone)
            .map_or(ptr::null_mut(), |clone| unsafe { clone(info, summary) });

        // SAFETY: `copy` is NULL or a live row; both accessors check the type.
        unsafe {
            if let Some(keywords) = server_keywords(info.cast_mut()) {
                set_server_keywords(copy, keywords);
            }
        }
        copy
    })
}

/// The class our overrides chain up to.
///
/// `g_type_class_peek` of the parent type rather than of the instance's own
/// class: a further subclass of ours would make the second point back at these
/// same functions and recurse until the stack ran out — the same rule the
/// finalize trampoline in `jmap-backend-core` follows.
///
/// # Safety
///
/// An instance of this type must exist, which is what guarantees its parent's
/// class is initialised and alive.
unsafe fn parent_class<'a>() -> Option<&'a CamelMessageInfoClass> {
    // SAFETY: the contract above.
    unsafe { subclass::parent_class::<CamelMessageInfoClass>(JmapMessageInfo::parent_type()) }
}

/// Runs one vfunc body, refusing to run it at all on a row that is not ours.
///
/// The type check is not defensive about Camel — it only dispatches a class's
/// vfuncs on instances of that class — it is what makes the *bodies* above able
/// to reach the instance struct without a check of their own. The panic guard is
/// the rule every `extern "C"` in this repository follows: a Rust panic must
/// never cross into C.
fn guard_row<T>(
    what: &str,
    info: *mut CamelMessageInfo,
    fallback: T,
    body: impl FnOnce() -> T,
) -> T {
    // SAFETY: `info` is the argument Camel dispatched on, so it is NULL or a
    // live instance.
    if unsafe { JmapMessageInfo::borrow(info) }.is_none() {
        log_critical(&format!(
            "CamelJmapMessageInfo::{what} called on a row of another type"
        ));
        return fallback;
    }
    guard(what, fallback, body)
}

/// The summary row for one message, owned by the caller.
///
/// Never NULL in practice — `camel_message_info_new` is a `g_object_new` on a
/// concrete type — but the pointer is returned as Camel's own functions return
/// it rather than unwrapped into a reference, because its only caller hands it
/// straight back to a summary.
pub fn new_message_info(message: &MessageSummary) -> *mut CamelMessageInfo {
    // Constructed directly rather than through `camel_message_info_new`, which
    // reads the type to instantiate off a *summary* and has none to read here:
    // the row is built before it is added, and the summary it is added to
    // declares this same type ([`crate::summary`]), so the two answers agree by
    // being the same function.
    //
    // SAFETY: a variadic construct call on a registered type, with NULL for the
    // first property name — a row is constructed with none.
    let info = unsafe { g_object_new(message_info_type(), ptr::null::<gchar>()) }
        .cast::<CamelMessageInfo>();
    if info.is_null() {
        return info;
    }

    // Every setter below emits a property notification and marks the row as
    // having to reach the server, which is what [`without_queueing`] is for and
    // is why the whole build is inside it rather than only the two columns
    // `update_message_info` writes: a row that came out of a listing carries no
    // change of the user's, so it must not arrive on the folder's work list.
    // A fresh row's bit is unset, so restoring it leaves it unset.
    //
    // SAFETY: `info` is a fresh info this function owns; every string passed
    // is NUL-terminated and copied by the setter, and the references array is
    // handed over to `take_references` along with its ownership.
    unsafe {
        without_queueing(info, || {
            camel_message_info_set_uid(info, c_string(message.uid.as_str()).as_ptr());
            // Whether the two mutable columns "changed" is meaningless on a row
            // that did not exist a line ago; the whole row is the change, and
            // its caller reports it as an addition.
            let _ = update_message_info(info, message);

            set_string(
                info,
                camel_message_info_set_subject,
                message.subject.as_deref(),
            );
            set_string(
                info,
                camel_message_info_set_from,
                address_list(&message.from).as_deref(),
            );
            set_string(
                info,
                camel_message_info_set_to,
                address_list(&message.to).as_deref(),
            );
            set_string(
                info,
                camel_message_info_set_cc,
                address_list(&message.cc).as_deref(),
            );
            set_string(
                info,
                camel_message_info_set_preview,
                message.preview.as_deref(),
            );

            camel_message_info_set_size(info, message.size);
            camel_message_info_set_date_received(info, message.received_at.unwrap_or(0));
            camel_message_info_set_date_sent(info, message.sent_at.unwrap_or(0));

            if let Some(id) = &message.message_id {
                camel_message_info_set_message_id(info, message_id_digest(id));
            }
            let ancestors = references_array(&message.references);
            if !ancestors.is_null() {
                camel_message_info_take_references(info, ancestors);
            }
        });
    }

    info
}

/// Writes the columns a later listing of the same message may have changed.
///
/// Every property of a JMAP `Email` is immutable except `keywords` and
/// `mailboxIds` (RFC 8621 §4.1) — a message does not get a new subject, a new
/// sender or a new `Message-ID` — and the mailbox it is in is not a column of
/// the row, it is which folder the row is in. So a refresh is these two
/// columns and nothing else, which is both what the server can honestly be
/// said to have changed and what [`crate::summary`] rewrites when it meets a
/// row that is already there. Everything else is written once, by
/// [`new_message_info`], which calls this for the rest of the row.
///
/// Reports whether either column actually moved, which is Camel's own answer
/// rather than a comparison made here: both setters return whether they changed
/// the value, and the summary needs that to decide whether the row belongs in
/// the diff a refresh emits. Written as two statements and an `||` rather than
/// the other way round, because `||` short-circuits — and a row whose flags
/// moved must still have its labels written.
///
/// Written inside `without_queueing`, because a listing is the server
/// speaking and the bit it would otherwise set means the opposite.
///
/// ## What the listing is written *over*
///
/// Not the row, if the row still has something to say — see
/// `outstanding_change`. A refresh timer that goes off between the user's
/// click and the synchronisation brings back a listing the server made before
/// the click, and writing that whole would undo the click on screen and, worse,
/// leave the row claiming exactly what it remembers the server holding: the next
/// synchronisation would diff those two into nothing and the change would never
/// be sent. So what is written is the listing with the row's outstanding change
/// replayed on top of it.
///
/// The remembered set is renewed to the *listing* either way, never to what the
/// row ends up claiming. That is what keeps the two ends of the next difference
/// honest — and it is why a change the server has meanwhile made itself settles
/// rather than being sent again: replayed onto a listing that already has it,
/// it changes nothing, and the diff against the listing is then empty.
///
/// `attachments` is taken from the listing on both paths. It is the one bit of
/// the word that is not a keyword — RFC 8621 §4.1.1 has the server compute
/// `hasAttachment` — so the keyword set the replay works with cannot carry it.
///
/// # Safety
///
/// `info` must be a live message info.
pub unsafe fn update_message_info(info: *mut CamelMessageInfo, message: &MessageSummary) -> bool {
    let listed = Keywords::new(&message.flags, &message.tags);

    // SAFETY: the caller guarantees the info; the mask keeps the setter off the
    // bits this provider is not the authority on, and the flag set below is
    // handed over along with its ownership.
    unsafe {
        let (mut flags, tags) = listed.patched(&outstanding_change(info)).split();
        flags.attachments = message.flags.attachments;

        let (word, labels) = without_queueing(info, || {
            let word = camel_message_info_set_flags(info, FLAGS_FROM_JMAP, flags_word(&flags));
            let labels = set_user_flags(info, &tags);
            (word, labels)
        });
        // Renewed alongside them, and deliberately not part of the answer: what
        // the server was last seen holding is not a column the message list
        // draws, so a listing that only re-spelled a keyword is not a change to
        // announce. A keyword the server really added arrives as a flag or a
        // label too, and is reported as one of those.
        set_server_keywords(info, listed);
        word != GFALSE || labels != GFALSE
    }
}

/// The change of the user's this row is still carrying — what a listing has to
/// be written *around* rather than over.
///
/// The two ends are the row's own two columns, and they are the same two
/// [`crate::synchronize`] takes: the keywords the last listing found and the
/// keywords the row claims now. So this is precisely the patch the next
/// synchronisation would send, read as something still owed rather than
/// something to make happen.
///
/// Empty for a row Camel does not hold as needing to reach the server, and the
/// bit is what decides it rather than the two sets: a row with nothing
/// outstanding takes the server's answer whole, which is the ordinary case and
/// the only thing that ever brings a row that has drifted back into line. A row
/// whose sets happen to differ with the bit clear is one whose difference nobody
/// is waiting to send.
///
/// # Safety
///
/// `info` must be a live message info.
unsafe fn outstanding_change(info: *mut CamelMessageInfo) -> KeywordChange {
    // SAFETY: the contract above; both accessors read the row's own columns.
    unsafe {
        if camel_message_info_get_folder_flagged(info) == GFALSE {
            return KeywordChange::default();
        }
        KeywordChange::between(
            &server_keywords(info).unwrap_or_default(),
            &row_keywords(info),
        )
    }
}

/// Takes a row off the folder's work list.
///
/// For the caller that has just put it on without meaning to.
/// `camel_folder_summary_add` marks every row it is handed as having to reach
/// the server, which is right for the caller it was written for — appending a
/// message the user composed — and wrong for [`crate::summary`], which adds
/// rows the server has just described. A row added by a listing has no change of
/// the user's on it by definition: it did not exist a moment ago.
///
/// Separate from [`without_queueing`] because the two answer different
/// questions. That one preserves whatever was there, for a row that already
/// existed and may already have been waiting; this one asserts there is nothing
/// to wait for.
///
/// # Safety
///
/// `info` must be a live message info.
pub(crate) unsafe fn clear_pending_write(info: *mut CamelMessageInfo) {
    // SAFETY: the contract above.
    unsafe { camel_message_info_set_folder_flagged(info, GFALSE) };
}

/// Writes a row's columns from a listing, leaving its place on the folder's
/// work list exactly as it was found.
///
/// Every one of Camel's column setters marks the row `FOLDER_FLAGGED` when it
/// changes anything. The bit means "this row has to reach the server", which is
/// right for the setter's usual caller — the user clicking on a message — and
/// exactly backwards here: this is the server *telling* us. A listing that left
/// the bit behind would put every row it wrote on the work list
/// [`crate::synchronize`] walks, so a refresh would queue the whole mailbox to
/// be written straight back to the server it came from.
///
/// Put back the way it was found rather than cleared, and the difference is the
/// row whose flags the user changed a moment before the refresh arrived. That
/// row is still waiting to be written, and clearing the bit would take it off
/// the work list and lose the change in silence. What the listing does to such a
/// row's columns is [`update_message_info`]'s side of the same question — it
/// replays the outstanding change over the listing rather than writing the
/// listing whole — and the two together are what let a refresh and a
/// synchronisation interleave without either one undoing the other.
///
/// The notifications are frozen around the write for a second reason, which is
/// the one Camel's own builders freeze for: a row rewritten column by column
/// while a summary watched would be a handful of separate changes to one
/// message. It is not what makes the restore work — the bit is set by each
/// setter as it runs, not on thaw — but the restore has to happen before the
/// thaw all the same, so that what the folder is told about is the row as it
/// ends up.
///
/// # Safety
///
/// `info` must be a live message info.
unsafe fn without_queueing<T>(info: *mut CamelMessageInfo, write: impl FnOnce() -> T) -> T {
    // SAFETY: the caller guarantees the info; the freeze and the thaw are a
    // matched pair, and the two flag calls are the same accessor read and
    // written.
    unsafe {
        camel_message_info_freeze_notifications(info);
        let queued = camel_message_info_get_folder_flagged(info);
        let written = write();
        camel_message_info_set_folder_flagged(info, queued);
        camel_message_info_thaw_notifications(info);
        written
    }
}

/// Replaces the row's user flags — Evolution's labels — with the ones the
/// listing carries.
///
/// Wholesale rather than name by name, because the keyword set is the whole
/// truth about a message's labels: a keyword the server has stopped sending is
/// one that was taken off somewhere else, and Camel is told about it only by
/// its absence. Handing over an empty set is therefore right rather than
/// merely harmless — unlike the text columns, where an empty value and an
/// absent one are different things in the summary database, user flags are
/// stored as one joined string with no way to spell "absent".
///
/// Reports Camel's own verdict on whether the set it was handed differed from
/// the one it had.
///
/// # Safety
///
/// `info` must be a live message info.
unsafe fn set_user_flags(info: *mut CamelMessageInfo, tags: &[String]) -> glib_sys::gboolean {
    // SAFETY: the set is built here and handed to `take_user_flags`, which
    // consumes it whether or not it turned out to be a change; every name is a
    // NUL-terminated string the set copies.
    unsafe {
        let flags = camel_named_flags_new();
        for tag in tags {
            camel_named_flags_insert(flags, c_string(tag).as_ptr());
        }
        camel_message_info_take_user_flags(info, flags)
    }
}

/// Sets one text column, or leaves it unset when the message did not carry it.
///
/// The distinction is not cosmetic: Camel writes the summary to a database and
/// reads it back, where an empty string is a header that was present and blank
/// rather than one that was absent.
///
/// # Safety
///
/// `info` must be a live message info, and `setter` one of Camel's own column
/// setters for it.
unsafe fn set_string(
    info: *mut CamelMessageInfo,
    setter: unsafe extern "C" fn(
        *mut CamelMessageInfo,
        *const glib_sys::gchar,
    ) -> glib_sys::gboolean,
    value: Option<&str>,
) {
    let Some(value) = value else { return };
    // SAFETY: the copy lives across the call, and the setter copies the string.
    unsafe { setter(info, c_string(value).as_ptr()) };
}

/// The bits of the flags word this provider is the authority on.
///
/// Passed as the mask to `camel_message_info_set_flags` so that a row being
/// refreshed keeps everything else Camel put in the word — `DELETED` and
/// `FOLDER_FLAGGED` are local marks the user made, and clearing them because
/// the server said nothing about them would undo a deletion the user is
/// waiting to have expunged.
const FLAGS_FROM_JMAP: CamelMessageFlags = CAMEL_MESSAGE_SEEN
    | CAMEL_MESSAGE_ANSWERED
    | CAMEL_MESSAGE_FLAGGED
    | CAMEL_MESSAGE_DRAFT
    | CAMEL_MESSAGE_FORWARDED
    | CAMEL_MESSAGE_JUNK
    | CAMEL_MESSAGE_NOTJUNK
    | CAMEL_MESSAGE_ATTACHMENTS;

/// The keywords Camel has a bit for, as that word.
///
/// One arm per field of [`MessageFlags`], which is one arm per bit the sync
/// layer can honestly speak to. Everything else in the word is Camel's:
/// `DELETED` because JMAP has no deleted keyword, `SECURE` and `ANSWERED_ALL`
/// because they are conclusions drawn from a message that has been fetched.
fn flags_word(flags: &MessageFlags) -> CamelMessageFlags {
    let mut word = 0;
    for (set, bit) in [
        (flags.seen, CAMEL_MESSAGE_SEEN),
        (flags.answered, CAMEL_MESSAGE_ANSWERED),
        (flags.flagged, CAMEL_MESSAGE_FLAGGED),
        (flags.draft, CAMEL_MESSAGE_DRAFT),
        (flags.forwarded, CAMEL_MESSAGE_FORWARDED),
        (flags.junk, CAMEL_MESSAGE_JUNK),
        (flags.not_junk, CAMEL_MESSAGE_NOTJUNK),
        (flags.attachments, CAMEL_MESSAGE_ATTACHMENTS),
    ] {
        if set {
            word |= bit;
        }
    }
    word
}

/// The same table read the other way: which of the keywords Camel has a bit for
/// this word says are set.
///
/// Written as one loop over the same pairs [`flags_word`] uses rather than as a
/// second list of them, because the two have to stay each other's inverse: a bit
/// named in one and not the other would be a flag the folder writes to the
/// server and never reads back, or reads back and never writes.
fn message_flags(word: CamelMessageFlags) -> MessageFlags {
    let mut flags = MessageFlags::default();
    for (field, bit) in [
        (&mut flags.seen, CAMEL_MESSAGE_SEEN),
        (&mut flags.answered, CAMEL_MESSAGE_ANSWERED),
        (&mut flags.flagged, CAMEL_MESSAGE_FLAGGED),
        (&mut flags.draft, CAMEL_MESSAGE_DRAFT),
        (&mut flags.forwarded, CAMEL_MESSAGE_FORWARDED),
        (&mut flags.junk, CAMEL_MESSAGE_JUNK),
        (&mut flags.not_junk, CAMEL_MESSAGE_NOTJUNK),
        (&mut flags.attachments, CAMEL_MESSAGE_ATTACHMENTS),
    ] {
        *field = word & bit != 0;
    }
    flags
}

/// One address header, in the single string Camel's summary keeps it as.
///
/// Built through `CamelInternetAddress` rather than by joining the parts here:
/// a display name may hold a comma, a quote or a backslash, and the rules for
/// which of those have to be quoted are RFC 5322's, already implemented once in
/// Camel. `None` for a header the message did not carry, which is not the same
/// as one Camel would store as empty.
fn address_list(addresses: &[EmailAddress]) -> Option<String> {
    if addresses.is_empty() {
        return None;
    }

    // SAFETY: the address object is constructed, filled here and released when
    // `address` drops at the end of the scope, and reaches nothing else; every
    // string handed over is NUL-terminated and copied, and `camel_address_format`
    // returns a `g_malloc`ed string this function frees.
    unsafe {
        let address = Owned::from_raw(camel_internet_address_new())?;
        for entry in addresses {
            let name = entry.name.as_deref().map(c_string);
            let email = c_string(&entry.email);
            camel_internet_address_add(
                address.as_ptr(),
                name.as_ref().map_or(ptr::null(), |name| name.as_ptr()),
                email.as_ptr(),
            );
        }

        let formatted = camel_address_format(address.as_ptr().cast::<CamelAddress>());
        if formatted.is_null() {
            return None;
        }
        let text = CStr::from_ptr(formatted).to_string_lossy().into_owned();
        g_free(formatted.cast());
        Some(text)
    }
}

/// The 64 bits Camel stores instead of a `Message-ID`.
///
/// `CamelSummaryMessageID` is a union over a `guint64` and eight bytes, and
/// Camel fills the bytes from the front of an MD5 over the header value with
/// the angle brackets already off — which is precisely what JMAP's `messageId`
/// property is. Reading the union back as the integer is therefore a
/// native-endian load of those eight bytes, and `from_ne_bytes` is that load
/// written down.
///
/// MD5 is not a security decision here and is not ours to make: it is the
/// digest already in every Camel summary on disk, and the only thing this value
/// is ever compared against is another one of them.
fn message_id_digest(message_id: &str) -> u64 {
    let mut digest = [0u8; 16];
    let mut length = digest.len();

    // SAFETY: the checksum is created, updated and freed here; the buffer is
    // ours and `length` is its true size, which is what `g_checksum_get_digest`
    // requires (it aborts on a buffer too small for the algorithm).
    unsafe {
        let checksum = g_checksum_new(G_CHECKSUM_MD5);
        g_checksum_update(checksum, message_id.as_ptr(), message_id.len() as isize);
        g_checksum_get_digest(checksum, digest.as_mut_ptr(), &mut length);
        g_checksum_free(checksum);
    }

    let (head, _) = digest.split_at(size_of::<u64>());
    u64::from_ne_bytes(
        head.try_into()
            .expect("eight bytes of a sixteen-byte digest"),
    )
}

/// The ancestors, as the `GArray` of digests Camel threads on — or NULL for a
/// message with none, because an empty array is one the threader walks for
/// nothing on every rebuild.
///
/// Reversed on the way in. `references` arrives oldest first, with the
/// `In-Reply-To` parent last, which is the order the headers put it in;
/// Camel's own builder stores the chain the other way round, nearest ancestor
/// first, and its threader walks from the front taking the first ancestor the
/// folder actually holds. Filled in header order, every reply in a long thread
/// would hang off the root of its conversation instead of off its parent.
///
/// The array is returned with its ownership, for `take_references`.
fn references_array(references: &[String]) -> *mut GArray {
    if references.is_empty() {
        return ptr::null_mut();
    }

    // SAFETY: the array is allocated here with `guint64` elements, which is
    // what `camel_message_info_take_references` documents them to be, and each
    // append copies eight bytes out of a local.
    unsafe {
        let array = g_array_sized_new(
            GFALSE,
            GFALSE,
            size_of::<u64>() as u32,
            references.len() as u32,
        );
        for reference in references.iter().rev() {
            let digest = message_id_digest(reference);
            g_array_append_vals(array, ptr::from_ref(&digest).cast(), 1);
        }
        array
    }
}
