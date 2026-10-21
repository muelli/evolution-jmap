// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelJmapMessageInfo`: one summary row, in the object form that Camel keeps.
//!
//! The [`MessageSummary`] of `jmap-mail-sync` is the same row, read from an
//! `Email/get`. This module copies most of its columns. Three columns are not
//! a copy:
//!
//! - **The flags word.** Camel keeps the state of a message as bits in one
//!   `guint32`. JMAP keeps it as a set of keywords. The sync layer divides the
//!   keywords into two groups: keywords that have a Camel bit, and keywords
//!   that do not. `flags_word` turns the first group into the word. The second
//!   group becomes the Camel user flags, which Evolution shows as labels.
//! - **The address columns.** JMAP sends `from`, `to` and `cc` as structured
//!   name/address pairs. The Camel summary keeps one display string per header.
//!   `CamelInternetAddress` formats that string and applies the RFC 5322
//!   quoting rules, so `address_list` builds one and uses it.
//! - **The threading columns.** Camel does not keep a `Message-ID`. It keeps
//!   64 bits of an MD5 of it (`CamelSummaryMessageID`), and an array of these
//!   digests for the ancestors. No public function computes a digest, so
//!   `message_id_digest` computes it. Its result must be equal to the result of
//!   `camel_message_info_new_from_headers`: a message that Camel parses locally
//!   goes into the same summary as a row from this module. Different digests
//!   would show one conversation as two threads. `tests/message_info.rs`
//!   compares the result with Camel itself, not with a constant.
//!
//! A fourth column is not a copy and not a computation. Only this provider
//! knows its value, so Camel has no field for it. Thus the row must be a
//! subclass.
//!
//! ## The keywords that the server held at the last listing
//!
//! On the wire, a flag change is a *difference* (`KeywordChange` in
//! `jmap-mail-sync`). A write of the full set would also speak for keywords
//! that no client here knows. A difference needs two ends, but the row has only
//! one. When the user marks a message read, the flags word changes in place:
//! that is the *after*. The *before* (the keywords from the last listing) is
//! lost at that moment, unless the row keeps it.
//!
//! Thus [`JmapMessageInfo`] is a `CamelMessageInfoBase` that also holds the
//! keyword set of the listing. [`new_message_info`] writes the set and
//! [`update_message_info`] renews it. The IMAPX message info has the same shape
//! for the same reason: it keeps a `server_flags` word next to the Camel flags.
//! The column survives a restart through the only method that Camel gives a
//! provider: the `bdata` of the summary database row. `bdata` is one string per
//! row. Each class in the chain appends to it on save, and reads it back in the
//! same order on load. (The row struct is `CamelMIRecord` up to EDS 3.52 and
//! `CamelStoreDBMessageRecord` from 3.58. Only the name changed, so the two
//! vfuncs below use [`eds_sys::compat::CamelSummaryMessageRecord`] and are the
//! same functions on both.)
//!
//! The purpose of the column gives two rules. Tests examine both:
//!
//! - **A row without the column remembers nothing, and loads correctly.** The
//!   empty set is the safe answer: a difference from nothing only adds
//!   keywords. Thus a row whose stored data is older than this column removes
//!   no keywords.
//! - **A clone into a summary of this provider keeps the column.** Without it,
//!   the next flag change of the copy would look like the removal of all the
//!   keywords of the message. The target summary of the copy decides whether
//!   the copy is a row of this type (see `clone`).
//!
//! The row is still built detached, with no summary: the column does not need
//! one. But the class that a *summary* instantiates is a separate question. The
//! subclass in [`crate::summary`] declares this type, so a row read back from
//! the database has this type. `tests/summary.rs` makes sure that the two
//! answers agree.
//!
//! ## The thread id from the server
//!
//! A fifth column, stored in the same way for the same reason. Camel has no
//! field for a thread that the server assigns, only its own guess from digests
//! (`message_id_digest`, above). The thread id does not change: RFC 8621
//! §4.1.1 does not let a message change thread. Thus [`new_message_info`] is
//! the only function that sets it from a listing, and `update_message_info`
//! does not write it. (`load` and `clone` only restore or copy it.) It is
//! stored, not only held in [`MessageSummary`], so that it survives a restart
//! as the keywords column does. The summary does not yet use it to group
//! messages into threads.

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
use jmap_proto::Id;
use jmap_proto::mail::EmailAddress;

use crate::folder_info::c_string;

/// The instance struct: the Camel row, plus the data that Camel has no field
/// for.
#[repr(C)]
pub struct JmapMessageInfo {
    parent: CamelMessageInfoBase,
    /// The keywords that the last listing found on the message.
    ///
    /// A [`Slot`], as all instance fields in this crate: the struct arrives
    /// zeroed and is freed without a destructor. A [`Mutex`] because, unlike
    /// the mailbox id of a folder, this field gets many writes: each refresh
    /// renews it, and Camel uses a folder from more than one thread.
    server: Slot<Mutex<Keywords>>,
    /// The thread id that `Email/get` gave for this message, or `None` if this
    /// provider has no thread id for the row.
    thread: Slot<Mutex<Option<Id>>>,
}

impl JmapMessageInfo {
    /// The Rust view of a row from Camel, or `None` if the row is not of this
    /// type.
    ///
    /// This function checks the type. A vfunc can assume the type of its first
    /// argument, but a row comes to this crate from a summary. A summary written
    /// before this type existed gives a plain `CamelMessageInfoBase`. To read
    /// such a row as this type is undefined behaviour.
    ///
    /// # Safety
    ///
    /// `info` must be NULL or point at a live `CamelMessageInfo`.
    unsafe fn borrow<'a>(info: *mut CamelMessageInfo) -> Option<&'a Self> {
        // SAFETY: the contract above makes the type check legal, and the
        // check makes the cast inside sound.
        unsafe { checked_borrow(info, message_info_type()) }
    }
}

/// The class struct: the vfunc slots of the parent, with functions of this
/// module in three of them.
#[repr(C)]
pub struct JmapMessageInfoClass {
    parent_class: CamelMessageInfoBaseClass,
}

// SAFETY: both structs are #[repr(C)] and start with the CamelMessageInfoBase
// instance and class structs. The eds-sys test tests/layout.rs checks their
// layouts against `g_type_query`. CamelMessageInfoBase derives from
// CamelMessageInfo, which derives from GObject.
unsafe impl ObjectSubclass for JmapMessageInfo {
    /// `CamelJmapMessageInfo`, as the store and the folder: the Camel
    /// providers use the name `Camel<Protocol>MessageInfo`.
    const NAME: &'static CStr = c"CamelJmapMessageInfo";
    type Instance = JmapMessageInfo;
    type Class = JmapMessageInfoClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { camel_message_info_base_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: the class starts with CamelMessageInfoBaseClass, which
        // starts with CamelMessageInfoClass (the contract above).
        let class = unsafe { &mut (*class).parent_class.parent_class };
        class.load = Some(load);
        class.save = Some(save);
        class.clone = Some(clone);
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // Fill the slot now. An empty slot and a slot with an empty set are
        // different answers: an empty slot is a row that this crate cannot
        // speak for, and all readers treat it so.
        //
        // SAFETY: the instance is in construction, so this is the only
        // reference to it.
        unsafe { (*instance).server.init(Mutex::new(Keywords::default())) };
        // SAFETY: as above.
        unsafe { (*instance).thread.init(Mutex::new(None)) };
    }

    unsafe fn finalize(instance: *mut Self::Instance) {
        // SAFETY: the instance is in finalization, so nothing can still get
        // to it and no borrow from `get` is alive. Without this, the set
        // leaks once for each row that the folder listed.
        unsafe { (*instance).server.clear() };
        // SAFETY: as above.
        unsafe { (*instance).thread.clear() };
    }
}

/// Registers the row type, or returns it if it is already registered.
///
/// The registration is static, as for the store and the folder: a Camel
/// provider is not a `GTypeModule`, so it never unloads a dynamic type.
pub fn message_info_type() -> GType {
    register_static::<JmapMessageInfo>()
}

/// The keywords that the server held when `info` last saw them, or `None` if
/// this provider did not build the row.
///
/// The return value is a copy, not a borrow. The set is behind a mutex, and
/// the row gives no borrow from it. Thus a refresh that renews the set cannot
/// interleave with a synchronisation that reads it.
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

/// The keyword set that the row shows *now*: the other end of a flag change.
///
/// [`server_keywords`] is the before and this is the after. The before is a
/// column that only this provider keeps. The after comes from two Camel
/// columns, the flags word and the user flags, because the click of the user
/// changes those. This function is `flags_word` and `set_user_flags` in
/// reverse, over the same bits and names. Thus an unchanged row gives the set
/// that built it, and no change.
///
/// The function reads `attachments` from the word for symmetry, but it has no
/// effect: [`Keywords::new`] ignores it. RFC 8621 §4.1.4 makes `hasAttachment`
/// a property that the server computes, not a label that a client can set.
///
/// # Safety
///
/// `info` must point at a live `CamelMessageInfo`.
pub unsafe fn row_keywords(info: *mut CamelMessageInfo) -> Keywords {
    // SAFETY: the contract above. `dup_user_flags` gives a copy that this
    // function owns and frees. Thus it is safe to read the copy without the
    // property lock of the row during the walk.
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

/// Records the result of one listing, and replaces the set that the row had.
///
/// The function replaces the full set, as for the user flags: a `keywords`
/// object is the full truth about the keywords of the message. Thus a keyword
/// that the listing does not include is no longer on the message.
///
/// For a row of a different type (a row from a summary that is older than this
/// column), the function does nothing. That is the same as an empty set: the
/// difference from nothing adds keywords and removes none.
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

/// The thread id that the server assigned to `info`, or `None` if this
/// provider has no thread id for the row.
///
/// # Safety
///
/// `info` must be NULL or point at a live `CamelMessageInfo`.
pub unsafe fn server_thread_id(info: *mut CamelMessageInfo) -> Option<Id> {
    // SAFETY: the contract above, and `borrow` checks the type.
    let row = unsafe { JmapMessageInfo::borrow(info) }?;
    row.thread
        .get()?
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Records the thread id of the row: from the listing in [`new_message_info`],
/// or from the stored data or the original row in `load` and `clone`.
///
/// For a row of a different type, the function does nothing. This is the same
/// fallback that [`set_server_keywords`] uses for a row from a summary that is
/// older than this column.
///
/// # Safety
///
/// `info` must be NULL or point at a live `CamelMessageInfo`.
unsafe fn set_server_thread_id(info: *mut CamelMessageInfo, thread_id: Option<Id>) {
    // SAFETY: the contract above, and `borrow` checks the type.
    let Some(row) = (unsafe { JmapMessageInfo::borrow(info) }) else {
        return;
    };
    if let Some(slot) = row.thread.get() {
        *slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = thread_id;
    }
}

/// `CamelMessageInfoClass.save`: appends the two columns to the stored data of
/// the row.
///
/// The keyword count comes first, then the names, because [`load`] reads this
/// format. The function writes each name with `camel_util_bdata_put_string`
/// and does not join them. `bdata` is one string for the full class chain, and
/// a keyword can contain a space, a digit or a dash. The length-prefixed Camel
/// encoding prevents a keyword from coming back as two.
///
/// # Safety
///
/// Camel calls this with a live row of this type, a record to fill and the
/// string that the chain builds.
unsafe extern "C" fn save(
    info: *const CamelMessageInfo,
    record: *mut CamelSummaryMessageRecord,
    bdata: *mut GString,
) -> gboolean {
    guard_row("save", info.cast_mut(), GFALSE, || {
        // SAFETY: a chain-up to the save of the parent, on an instance of a
        // type derived from it, with the unchanged arguments from Camel.
        let chained = unsafe { parent_class() }
            .and_then(|class| class.save)
            .map_or(GFALSE, |save| unsafe { save(info, record, bdata) });
        if chained == GFALSE {
            return GFALSE;
        }

        // SAFETY: `info` is a row of this type, so the set is there. `bdata`
        // is the live GString that Camel builds. Each name is NUL-terminated,
        // and the call copies it.
        unsafe {
            let keywords = server_keywords(info.cast_mut()).unwrap_or_default();
            camel_util_bdata_put_number(bdata, keywords.len() as i64);
            for name in keywords.iter() {
                camel_util_bdata_put_string(bdata, c_string(name).as_ptr());
            }
        }

        // SAFETY: `info` is a row of this type, so the field is there. `bdata`
        // is the same live GString that has the keywords.
        unsafe {
            match server_thread_id(info.cast_mut()) {
                Some(id) => {
                    camel_util_bdata_put_number(bdata, 1);
                    camel_util_bdata_put_string(bdata, c_string(id.as_str()).as_ptr());
                }
                None => camel_util_bdata_put_number(bdata, 0),
            }
        }
        GTRUE
    })
}

/// `CamelMessageInfoClass.load`: reads the two columns back from the stored
/// data.
///
/// If the count goes past the end of the string, the record is truncated or
/// foreign. This is not an error. The reads stop at the first missing name, and
/// the row keeps the keywords that *were* stored. A failure result would drop
/// the full row, with all its other columns, because of one optional column.
///
/// # Safety
///
/// Camel calls this with a live row of this type, its stored record, and the
/// cursor that the chain reads through.
unsafe extern "C" fn load(
    info: *mut CamelMessageInfo,
    record: *const CamelSummaryMessageRecord,
    cursor: *mut *mut gchar,
) -> gboolean {
    guard_row("load", info, GFALSE, || {
        // SAFETY: chain up first. The cursor reads in the order of the write,
        // and the fields of the parent come first.
        let chained = unsafe { parent_class() }
            .and_then(|class| class.load)
            .map_or(GFALSE, |load| unsafe { load(info, record, cursor) });
        if chained == GFALSE {
            return GFALSE;
        }

        // SAFETY: the cursor belongs to Camel and points into the `bdata` of
        // the record. `get_string` moves it forward and returns a `g_malloc`ed
        // string, or NULL when no data remains.
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

        // SAFETY: `info` is a row of this type, because of the guard above.
        unsafe { set_server_keywords(info, names.into_iter().collect()) };

        // SAFETY: after the reads above, the cursor is immediately after the
        // keywords column. `save` wrote the thread-id column at that position.
        let thread_id = unsafe {
            let present = camel_util_bdata_get_number(cursor, 0) != 0;
            present
                .then(|| camel_util_bdata_get_string(cursor, ptr::null()))
                .filter(|raw| !raw.is_null())
                .map(|raw| {
                    let text = CStr::from_ptr(raw).to_string_lossy().into_owned();
                    g_free(raw.cast());
                    Id::new(text)
                })
        };
        // SAFETY: `info` is a row of this type, because of the guard above.
        unsafe { set_server_thread_id(info, thread_id) };
        GTRUE
    })
}

/// `CamelMessageInfoClass.clone`: gives the two columns to the copy.
///
/// This function does not select which copies get them. The clone of the parent
/// builds its result from the *summary* of the copy: it is
/// `camel_message_info_new` on that summary. Thus a row cloned into a summary of
/// this provider comes back as this type. A row cloned into no summary comes
/// back as a plain `CamelMessageInfoBase`, and this function does not change it.
/// For this reason, the write uses the checked accessor, not the instance
/// struct. A copy of a different type is in a folder that has no JMAP keywords.
///
/// To override the full clone and force the type, this function must rebuild
/// all columns of the row. That is the work of the parent, and the override
/// would silently not copy a column that Camel adds in the future.
///
/// # Safety
///
/// Camel calls this with a live row of this type.
unsafe extern "C" fn clone(
    info: *const CamelMessageInfo,
    summary: *mut CamelFolderSummary,
) -> *mut CamelMessageInfo {
    guard_row("clone", info.cast_mut(), ptr::null_mut(), || {
        // SAFETY: a chain-up to the clone of the parent with the arguments from
        // Camel. The caller owns the row that it returns.
        let copy = unsafe { parent_class() }
            .and_then(|class| class.clone)
            .map_or(ptr::null_mut(), |clone| unsafe { clone(info, summary) });

        // SAFETY: `copy` is NULL or a live row, and both accessors check the
        // type.
        unsafe {
            if let Some(keywords) = server_keywords(info.cast_mut()) {
                set_server_keywords(copy, keywords);
            }
            if let Some(thread_id) = server_thread_id(info.cast_mut()) {
                set_server_thread_id(copy, Some(thread_id));
            }
        }
        copy
    })
}

/// The class that the overrides of this module chain up to.
///
/// This is `g_type_class_peek` of the parent type, not the class of the
/// instance. With a subclass of this type, the class of the instance points
/// back at these same functions, and the recursion overflows the stack. The
/// finalize trampoline in `jmap-backend-core` uses the same rule.
///
/// # Safety
///
/// An instance of this type must exist. This makes sure that the class of its
/// parent is initialised and alive.
unsafe fn parent_class<'a>() -> Option<&'a CamelMessageInfoClass> {
    // SAFETY: the contract above.
    unsafe { subclass::parent_class::<CamelMessageInfoClass>(JmapMessageInfo::parent_type()) }
}

/// Runs one vfunc body, but not on a row of a different type.
///
/// The type check is not a defence against Camel, which dispatches the vfuncs
/// of a class only on instances of that class. The check lets the *bodies*
/// above use the instance struct without a check of their own. The panic guard
/// applies the rule for all `extern "C"` functions in this repository: a Rust
/// panic must never cross into C.
fn guard_row<T>(
    what: &str,
    info: *mut CamelMessageInfo,
    fallback: T,
    body: impl FnOnce() -> T,
) -> T {
    // SAFETY: `info` is the argument that Camel dispatched on, so it is NULL or
    // a live instance.
    if unsafe { JmapMessageInfo::borrow(info) }.is_none() {
        log_critical(&format!(
            "CamelJmapMessageInfo::{what} called on a row of another type"
        ));
        return fallback;
    }
    guard(what, fallback, body)
}

/// The summary row for one message. The caller owns it.
///
/// It is never NULL in practice: `g_object_new` on a concrete type does not
/// fail. But the function returns the pointer as the Camel functions do, not as
/// a reference, because its only caller gives it directly to a summary.
pub fn new_message_info(message: &MessageSummary) -> *mut CamelMessageInfo {
    // Construct the row directly, not through `camel_message_info_new`. That
    // function reads the type from a *summary*, and there is no summary here:
    // the row is built before it is added. The summary that gets the row
    // declares this same type ([`crate::summary`]), so the two answers agree.
    //
    // SAFETY: a variadic construct call on a registered type, with NULL for the
    // first property name: a row has no construct properties.
    let info = unsafe { g_object_new(message_info_type(), ptr::null::<gchar>()) }
        .cast::<CamelMessageInfo>();
    if info.is_null() {
        return info;
    }

    // Each setter below emits a property notification and marks the row as
    // waiting for the server. [`without_queueing`] prevents this, so the full
    // build is inside it, not only the two columns of `update_message_info`.
    // A row from a listing has no change from the user, so it must not go on
    // the work list of the folder. The bit of a new row is clear, so the
    // restore keeps it clear.
    //
    // SAFETY: `info` is a new info that this function owns. Each string is
    // NUL-terminated, and the setter copies it. `take_references` gets the
    // references array and its ownership.
    unsafe {
        without_queueing(info, || {
            camel_message_info_set_uid(info, c_string(message.uid.as_str()).as_ptr());
            // The "changed" result is not meaningful for a new row. The full
            // row is the change, and the caller reports it as an addition.
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
            set_server_thread_id(info, message.thread_id.clone());
        });
    }

    info
}

/// Writes the columns that a later listing of the same message can change.
///
/// All properties of a JMAP `Email` are immutable, except `keywords` and
/// `mailboxIds` (RFC 8621 §4.1). A message never gets a new subject, sender
/// or `Message-ID`. The mailbox is not a column of the row: it is
/// the folder that holds the row. Thus a refresh writes only the flags and the
/// labels. The server can change only these, and [`crate::summary`] rewrites
/// only these for a row that exists. [`new_message_info`] writes all other
/// columns once, and calls this function for these two.
///
/// Returns whether one of the two columns changed. The two Camel setters give
/// this answer: each returns whether it changed the value. The summary uses it
/// to decide whether the row goes into the diff of the refresh. The two setters
/// run as two statements before the `||`, because `||` short-circuits: if the
/// flags change, the labels must still be written.
///
/// The writes are inside `without_queueing`. A listing comes from the server,
/// and the bit that the setters set means the opposite.
///
/// ## The data below the listing
///
/// If the row has a change that is not sent yet, the listing does not replace
/// it (see `outstanding_change`). A refresh timer can fire between the click of
/// the user and the synchronisation. Its listing is from before the click. To
/// write that listing as it is would undo the click on screen. Also, the row
/// would then show the same set that it remembers from the server. The next
/// synchronisation would find no difference, and would never send the change.
/// Thus the function writes the listing with the outstanding change applied
/// again on top.
///
/// In both cases, the remembered set becomes the *listing*, never the set that
/// the row shows. This keeps the two ends of the next difference correct. Thus
/// a change that the server already has does not go again: applied to a
/// listing that has it, it changes nothing, and the diff is empty.
///
/// `attachments` comes from the listing in both cases. It is the only bit of the
/// word that is not a keyword (RFC 8621 §4.1.4 makes the server compute
/// `hasAttachment`), so the keyword set of the replay cannot hold it.
///
/// # Safety
///
/// `info` must be a live message info.
pub unsafe fn update_message_info(info: *mut CamelMessageInfo, message: &MessageSummary) -> bool {
    let listed = Keywords::new(&message.flags, &message.tags);

    // SAFETY: the caller guarantees the info. The mask keeps the setter away
    // from the bits that this provider does not control. `set_user_flags`
    // gives its flag set to Camel with its ownership.
    unsafe {
        let (mut flags, tags) = listed.patched(&outstanding_change(info)).split();
        flags.attachments = message.flags.attachments;

        let (word, labels) = without_queueing(info, || {
            let word = camel_message_info_set_flags(info, FLAGS_FROM_JMAP, flags_word(&flags));
            let labels = set_user_flags(info, &tags);
            (word, labels)
        });
        // Renew the remembered set too, but do not include it in the result.
        // The message list does not show it, so a listing that only spells a
        // keyword differently is not a change. A keyword that the server
        // really added also changes a flag or a label, and the result
        // includes that change.
        set_server_keywords(info, listed);
        word != GFALSE || labels != GFALSE
    }
}

/// The user change that the row still holds: a listing must keep it, not
/// replace it.
///
/// The two ends are the two columns of the row, which are also the two that
/// [`crate::synchronize`] uses: the keywords of the last listing and the
/// keywords that the row shows now. Thus this is the same patch that the next
/// synchronisation sends, read as a change that is still waiting.
///
/// The result is empty if Camel does not mark the row as waiting for the
/// server. The bit decides, not the two sets. A row with no waiting change
/// takes the full answer from the server. This is the usual case, and the only
/// way that a row with old data becomes correct again. If the sets differ but
/// the bit is clear, nothing waits to send that difference.
///
/// # Safety
///
/// `info` must be a live message info.
unsafe fn outstanding_change(info: *mut CamelMessageInfo) -> KeywordChange {
    // SAFETY: the contract above. Both accessors read the columns of the row.
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

/// Removes a row from the work list of the folder.
///
/// Use it after an unwanted add to the work list. `camel_folder_summary_add`
/// marks each row as waiting for the server. That is correct for its usual
/// caller, which appends a message that the user wrote. It is not correct for
/// [`crate::summary`], which adds rows that the server just sent. A row from a
/// listing has no user change: it did not exist a moment before.
///
/// This is not [`without_queueing`], because the two have different purposes.
/// That function keeps the bit as it was, for a row that existed and possibly
/// waits already. This function sets the bit to "nothing waits".
///
/// # Safety
///
/// `info` must be a live message info.
pub(crate) unsafe fn clear_pending_write(info: *mut CamelMessageInfo) {
    // SAFETY: the contract above.
    unsafe { camel_message_info_set_folder_flagged(info, GFALSE) };
}

/// Writes the columns of a row from a listing, and keeps the position of the
/// row on the work list of the folder unchanged.
///
/// Each Camel column setter sets `FOLDER_FLAGGED` on the row when it changes a
/// value. The bit means "this row must go to the server". That is correct for
/// the usual caller of the setter (a user click on a message), but here the
/// data *comes* from the server. If a listing kept the bit, each row that it
/// wrote would go on the work list that [`crate::synchronize`] walks. A refresh
/// would then send the full mailbox back to the server.
///
/// The function restores the bit, and does not clear it. The difference is
/// important for a row whose flags the user changed just before the refresh.
/// That row still waits for its write. To clear the bit would remove it from the
/// work list and silently lose the change. [`update_message_info`] handles the
/// columns of such a row: it applies the outstanding change on top of the
/// listing. Together, the two functions let a refresh and a synchronisation
/// interleave, and neither undoes the other.
///
/// The function also freezes the notifications during the write, for the same
/// reason as the Camel builders: a column-by-column rewrite would show as many
/// separate changes to one message. The freeze does not make the restore work,
/// because each setter sets the bit when it runs, not on thaw. But the restore
/// must occur before the thaw, so that the folder gets the final state of the
/// row.
///
/// # Safety
///
/// `info` must be a live message info.
unsafe fn without_queueing<T>(info: *mut CamelMessageInfo, write: impl FnOnce() -> T) -> T {
    // SAFETY: the caller guarantees the info. The freeze and the thaw are a
    // matched pair, and the two flag calls read and write the same accessor.
    unsafe {
        camel_message_info_freeze_notifications(info);
        let queued = camel_message_info_get_folder_flagged(info);
        let written = write();
        camel_message_info_set_folder_flagged(info, queued);
        camel_message_info_thaw_notifications(info);
        written
    }
}

/// Replaces the user flags of the row (the Evolution labels) with the labels of
/// the listing.
///
/// The function replaces the full set, not each name, because the keyword set
/// is the full truth about the labels of a message. If the server stops sending
/// a keyword, another client removed it, and only its absence tells Camel. Thus
/// an empty set is correct. The text columns are different: there, the summary
/// database keeps an empty value and an absent value as different things. User
/// flags are one joined string, which cannot show "absent".
///
/// Returns the Camel result: whether the new set is different from the old set.
///
/// # Safety
///
/// `info` must be a live message info.
unsafe fn set_user_flags(info: *mut CamelMessageInfo, tags: &[String]) -> glib_sys::gboolean {
    // SAFETY: this function builds the set and gives it to `take_user_flags`,
    // which consumes it in all cases. Each name is a NUL-terminated string, and
    // the set copies it.
    unsafe {
        let flags = camel_named_flags_new();
        for tag in tags {
            camel_named_flags_insert(flags, c_string(tag).as_ptr());
        }
        camel_message_info_take_user_flags(info, flags)
    }
}

/// Sets one text column, or keeps it unset if the message does not have it.
///
/// The difference is important. Camel writes the summary to a database and
/// reads it back. There, an empty string is a header that was present and
/// blank, not an absent header.
///
/// # Safety
///
/// `info` must be a live message info, and `setter` must be a Camel column
/// setter for it.
unsafe fn set_string(
    info: *mut CamelMessageInfo,
    setter: unsafe extern "C" fn(
        *mut CamelMessageInfo,
        *const glib_sys::gchar,
    ) -> glib_sys::gboolean,
    value: Option<&str>,
) {
    let Some(value) = value else { return };
    // SAFETY: the copy stays alive during the call, and the setter copies the
    // string.
    unsafe { setter(info, c_string(value).as_ptr()) };
}

/// The bits of the flags word that this provider controls.
///
/// This is the mask for `camel_message_info_set_flags`. Thus a refresh keeps
/// all other bits that Camel put in the word. `DELETED` and `FOLDER_FLAGGED`
/// are local marks. If a refresh cleared them because the server did not send
/// them, it would undo a deletion that waits for an expunge.
const FLAGS_FROM_JMAP: CamelMessageFlags = CAMEL_MESSAGE_SEEN
    | CAMEL_MESSAGE_ANSWERED
    | CAMEL_MESSAGE_FLAGGED
    | CAMEL_MESSAGE_DRAFT
    | CAMEL_MESSAGE_FORWARDED
    | CAMEL_MESSAGE_JUNK
    | CAMEL_MESSAGE_NOTJUNK
    | CAMEL_MESSAGE_ATTACHMENTS;

/// The keywords that have a Camel bit, as a flags word.
///
/// One pair for each field of [`MessageFlags`], which is one pair for each bit
/// that the sync layer controls. Camel controls all other bits: `DELETED`
/// because JMAP has no deleted keyword, `SECURE` and `ANSWERED_ALL` because
/// they come from the analysis of a fetched message.
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

/// The reverse of [`flags_word`]: the keywords with a Camel bit that this word
/// sets.
///
/// The list of pairs is a second copy of the list in `flags_word`. Keep the two
/// lists identical, so that each function is the inverse of the other. A bit
/// in only one list is a flag that the folder writes to the server and never
/// reads back, or the opposite.
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

/// One address header, as the single string that the Camel summary keeps.
///
/// The function uses `CamelInternetAddress` and does not join the parts here.
/// A display name can contain a comma, a quote or a backslash. RFC 5322 gives
/// the quoting rules for these, and Camel already implements them. `None` for a
/// header that the message does not have. This is different from a header that
/// Camel keeps as empty.
fn address_list(addresses: &[EmailAddress]) -> Option<String> {
    if addresses.is_empty() {
        return None;
    }

    // SAFETY: this scope constructs and fills the address object, and releases
    // it when `address` drops at the end. Nothing else gets the object. Each
    // string is NUL-terminated and copied. `camel_address_format` returns a
    // `g_malloc`ed string that this function frees.
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

/// The 64 bits that Camel keeps in place of a `Message-ID`.
///
/// `CamelSummaryMessageID` is a union of a `guint64` and eight bytes. Camel
/// fills the bytes from the start of an MD5 of the header value without the
/// angle brackets. The JMAP `messageId` property is exactly that value. Thus
/// the union read as an integer is a native-endian load of those eight bytes,
/// which is what `from_ne_bytes` does.
///
/// MD5 is not a security decision here, and this module does not select it.
/// All Camel summaries on disk use this digest, and this value is compared only
/// with other values of the same kind.
fn message_id_digest(message_id: &str) -> u64 {
    let mut digest = [0u8; 16];
    let mut length = digest.len();

    // SAFETY: this block creates, updates and frees the checksum. The buffer
    // belongs to this function and `length` is its true size, as
    // `g_checksum_get_digest` requires (it refuses a buffer too small for the
    // algorithm and writes no digest).
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

/// The ancestors, as the `GArray` of digests that the Camel threader uses. NULL
/// for a message with no ancestors, because the threader walks an empty array
/// for no result at each rebuild.
///
/// The function reverses the order. `references` comes oldest first, with the
/// `In-Reply-To` parent last, as in the headers. The Camel builder keeps the
/// chain nearest ancestor first. Its threader starts at the front and takes the
/// first ancestor that the folder has. In header order, each reply in a long
/// thread would attach to the root of the conversation, not to its parent.
///
/// The caller gets the array and its ownership, for `take_references`.
fn references_array(references: &[String]) -> *mut GArray {
    if references.is_empty() {
        return ptr::null_mut();
    }

    // SAFETY: this block allocates the array with `guint64` elements, as the
    // documentation of `camel_message_info_take_references` specifies. Each
    // append copies eight bytes from a local.
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
