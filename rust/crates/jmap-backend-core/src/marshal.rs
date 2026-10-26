// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The parts of the vfunc boundary that are not about contacts or events.
//!
//! Every EDS meta backend reads strings that its caller owns. It writes
//! strings and lists into out-parameters that its caller frees. It gets the
//! password that EDS fetched from libsecret. These operations are the same
//! for the address book, the calendar and the mail store. Each of them can
//! cause an ownership error in the process of a different program, so they
//! are here, one time.
//!
//! These functions support the out-parameter rules that EDS uses:
//!
//! - The written value is a GLib allocation, and ownership goes to the
//!   caller. EDS frees an `out_new_sync_tag` with `g_free`. Thus a node that
//!   points into a Rust `String` would not be a leak: it would be a double
//!   free.
//! - A NULL out-parameter means "not interested", and the function skips it.
//! - A call that fails writes nothing, because EDS frees only the outputs of
//!   a call that succeeded.
//!
//! Only type-agnostic code is here. The `marshal` of each backend builds the
//! lists, because an `EBookMetaBackendInfo` and an `ECalMetaBackendInfo` are
//! different structs with different free functions.

use std::ffi::CStr;

use eds_sys::{
    E_CONFLICT_RESOLUTION_FAIL, E_CONFLICT_RESOLUTION_KEEP_SERVER, E_SOURCE_CREDENTIAL_PASSWORD,
    EConflictResolution, ENamedParameters, ESource, e_named_parameters_get, e_source_get_extension,
    e_source_has_extension,
};
use glib_sys::{GFALSE, GSList, GType, g_free, g_strdup, gchar};
use gobject_sys::g_type_check_instance_is_a;
use jmap_object_sync::OnConflict;

use crate::error::cstring_lossy;

/// The [`OnConflict`] for an `EConflictResolution`. The local save is the
/// newer value, so any value other than "keep server" or "fail" keeps it.
pub fn on_conflict(resolution: EConflictResolution) -> OnConflict {
    match resolution {
        E_CONFLICT_RESOLUTION_KEEP_SERVER => OnConflict::KeepServer,
        E_CONFLICT_RESOLUTION_FAIL => OnConflict::Fail,
        _ => OnConflict::KeepLocal,
    }
}

/// A C string that the caller owns, as an owned `Option<String>`.
///
/// "" is never a meaningful host, identifier or sync tag, so this function
/// reads it as absent. For the sync tag, this is important: the server would
/// get an empty tag as a state, not as no state. For the `ESource` fields,
/// the setters already normalise a cleared key to NULL. The callers must not
/// need to know that the two forms have the same meaning.
///
/// # Safety
///
/// `s` must be NULL or a valid NUL-terminated string that outlives the call.
pub unsafe fn read_string(s: *const gchar) -> Option<String> {
    if s.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees a valid NUL-terminated string.
    let value = unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned();
    (!value.is_empty()).then_some(value)
}

/// [`read_string`] for a string that the caller gives to this function
/// (`transfer full`). This function then frees the string.
///
/// # Safety
///
/// `s` must be NULL or a valid NUL-terminated string allocated by GLib.
pub unsafe fn take_string(s: *mut gchar) -> Option<String> {
    // SAFETY: NULL or a valid string, by the contract of this function.
    let value = unsafe { read_string(s) };
    // SAFETY: NULL or a GLib allocation that the caller owns. `g_free`
    // accepts NULL.
    unsafe { g_free(s.cast()) };
    value
}

/// Writes a copy of `value` into a `gchar **` out-parameter, which the caller
/// frees with `g_free`. A NULL `dest` is ignored.
///
/// # Safety
///
/// `dest` must be NULL or point at a writable `*mut gchar`.
pub unsafe fn set_out_string(dest: *mut *mut gchar, value: &str) {
    if dest.is_null() {
        return;
    }
    // SAFETY: `dest` is writable by the contract above. Ownership of the
    // duplicate goes through it to the caller.
    unsafe { *dest = dup_string(value) }
}

/// Writes a list into a `GSList **` out-parameter. It builds the list only
/// if the caller wants one. A list built for a NULL out-parameter would need
/// the correct per-node free function. To not build it is simpler than to
/// get that correct in two places.
///
/// # Safety
///
/// `dest` must be NULL or point at a writable `*mut GSList`. `build` must
/// return a list whose ownership can go to the caller.
pub unsafe fn set_out_list(dest: *mut *mut GSList, build: impl FnOnce() -> *mut GSList) {
    if !dest.is_null() {
        // SAFETY: `dest` is writable by the contract above.
        unsafe { *dest = build() };
    }
}

/// `g_strdup` of a Rust string. With this, a string gets into a list node
/// that EDS frees with `g_free`.
///
/// # Safety
///
/// The result is a GLib allocation. The caller must `g_free` it, or give it
/// to code that frees it.
pub unsafe fn dup_string(value: &str) -> *mut gchar {
    let text = cstring_lossy(value);
    // SAFETY: `text` is NUL-terminated and valid for the call.
    unsafe { g_strdup(text.as_ptr()) }
}

/// The Rust view of a Camel/GObject instance that dispatch already
/// guarantees.
///
/// GObject dispatches the vfuncs of a class only on an instance of that
/// class. Thus the first argument of `connect_sync`, `get_folder_sync` and
/// similar vfuncs needs no type check before the cast. A pointer from an
/// ordinary property or argument needs the check (use the `checked`-family
/// functions for it). This is only the trusted half: a bare null check, then
/// the cast.
///
/// # Safety
///
/// `ptr` must be NULL or point at a live instance of `T`.
pub unsafe fn dispatched_borrow<'a, C, T>(ptr: *mut C) -> Option<&'a T> {
    // SAFETY: the contract of the caller makes this cast sound.
    unsafe { ptr.cast::<T>().as_ref() }
}

/// The Rust view of a GObject instance from an ordinary property or
/// argument, not from vfunc dispatch. Thus, unlike [`dispatched_borrow`], the
/// class dispatch guarantee does not apply. The function must check `gtype`
/// before the cast is sound.
///
/// # Safety
///
/// `ptr` must be NULL or point at a live `GTypeInstance`.
pub unsafe fn checked_borrow<'a, C, T>(ptr: *mut C, gtype: GType) -> Option<&'a T> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: `ptr` is a live `GTypeInstance` by the contract of the caller.
    // The type check immediately before the cast makes the cast sound.
    unsafe {
        if g_type_check_instance_is_a(ptr.cast(), gtype) == GFALSE {
            return None;
        }
        ptr.cast::<T>().as_ref()
    }
}

/// As [`checked_borrow`], but returns the raw pointer, not a borrow. This is
/// for callers where `T` is a foreign C type that this crate only passes to
/// more C calls. It is not a Rust struct that can safely have a reference.
///
/// # Safety
///
/// `ptr` must be NULL or point at a live `GTypeInstance`.
pub unsafe fn checked_borrow_ptr<C, T>(ptr: *mut C, gtype: GType) -> Option<*mut T> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: see `checked_borrow`.
    unsafe {
        if g_type_check_instance_is_a(ptr.cast(), gtype) == GFALSE {
            return None;
        }
        Some(ptr.cast::<T>())
    }
}

/// As [`checked_borrow_ptr`], but for a caller that must distinguish a NULL
/// pointer (`Ok(None)`, "nothing here") from a pointer that failed the
/// `gtype` check (`Err(err)`). An example: a caller where an argument of the
/// wrong type is a refusal that the user sees, not the "absent" answer of a
/// NULL.
///
/// # Safety
///
/// `ptr` must be NULL or point at a live `GTypeInstance`.
pub unsafe fn checked_borrow_ptr_or<C, T, E>(
    ptr: *mut C,
    gtype: GType,
    err: E,
) -> Result<Option<*mut T>, E> {
    if ptr.is_null() {
        return Ok(None);
    }
    // SAFETY: see `checked_borrow`.
    unsafe {
        if g_type_check_instance_is_a(ptr.cast(), gtype) == GFALSE {
            return Err(err);
        }
        Ok(Some(ptr.cast::<T>()))
    }
}

/// Reads the extension `name` of `source`. If it is absent, this function
/// does not create it.
///
/// `e_source_get_extension` *creates* an extension that it cannot find. That
/// is the wrong answer when the caller only reads a source: a `.source`
/// keyfile that the caller does not own, or the file of an account that must
/// not get a new group only because code examined it. This function puts
/// the `e_source_has_extension` guard and the fetch into one call. Use it
/// when the only validation that the pointer needs is "the name of the
/// caller finds it".
///
/// # Safety
///
/// `source` must be a valid `ESource` that outlives the call. After its
/// registration, the extension that `name` names must be of type `T`. A
/// bare `.cast::<T>()` on the result of `e_source_get_extension` has the
/// same contract.
pub unsafe fn extension_if_present<T>(source: *mut ESource, name: &CStr) -> Option<*mut T> {
    // SAFETY: `source` is valid by the contract of the caller, and `name` is
    // NUL-terminated by its type.
    if unsafe { e_source_has_extension(source, name.as_ptr()) } == GFALSE {
        return None;
    }
    // SAFETY: the extension is present, so this returns the extension of the
    // source. The source owns it, and by the contract of the caller, it
    // outlives the call.
    Some(unsafe { e_source_get_extension(source, name.as_ptr()).cast::<T>() })
}

/// The password that EDS fetched from libsecret, if it has one now.
///
/// This function reports an empty stored password as present, not as absent.
/// This is why it is not [`read_string`]. With "absent", `connect_sync` would
/// ask EDS to prompt. A user who then enters nothing would get the prompt
/// again and again, without end. To send the empty password and get a
/// "wrong password" answer ends the loop.
///
/// # Safety
///
/// `credentials` must be NULL (EDS passes NULL before it asks libsecret for
/// anything), or a valid `ENamedParameters`.
pub unsafe fn password(credentials: *const ENamedParameters) -> Option<String> {
    if credentials.is_null() {
        return None;
    }
    // SAFETY: the name is a header constant. The parameters own the returned
    // string, and they outlive this call.
    let value = unsafe {
        e_named_parameters_get(credentials, E_SOURCE_CREDENTIAL_PASSWORD.as_ptr()).cast::<gchar>()
    };
    if value.is_null() {
        return None;
    }
    Some(
        // SAFETY: a non-NULL value is a NUL-terminated string that the
        // parameters own.
        unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned(),
    )
}
