// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The parts of the vfunc boundary that are not about contacts or events.
//!
//! Every EDS meta backend reads strings its caller owns, writes strings and
//! lists into out-parameters its caller will free, and is handed the password
//! EDS fetched from libsecret. None of that changes between the address book,
//! the calendar and the mail store, and all of it is a way to get the ownership
//! wrong in someone else's process, so it lives here once.
//!
//! The out-parameter discipline these support is the one EDS relies on:
//!
//! - what is written is a GLib allocation, ownership of which passes to the
//!   caller — EDS frees an `out_new_sync_tag` with `g_free`, so a node pointing
//!   into a Rust `String` would not be a leak, it would be a double free;
//! - a NULL out-parameter means "not interested" and is skipped;
//! - nothing is written by a call that fails, since EDS only frees the outputs
//!   of a call that succeeded.
//!
//! What stays here is only what is type-agnostic. The lists themselves are
//! built by each backend's own `marshal`, because an `EBookMetaBackendInfo` and
//! an `ECalMetaBackendInfo` are neither the same struct nor freed by the same
//! function.

use std::ffi::CStr;

use eds_sys::{
    E_SOURCE_CREDENTIAL_PASSWORD, ENamedParameters, ESource, e_named_parameters_get,
    e_source_get_extension, e_source_has_extension,
};
use glib_sys::{GFALSE, GSList, GType, g_strdup, gchar};
use gobject_sys::g_type_check_instance_is_a;

use crate::error::cstring_lossy;

/// A C string the caller owns, as an owned `Option<String>`.
///
/// "" is never a meaningful host, identifier or sync tag, so it reads as
/// absent. For the sync tag that is load-bearing: an empty tag would be sent to
/// the server as a state rather than as the absence of one, and for the
/// `ESource` fields the setters already normalise a cleared key to NULL — the
/// two spellings meaning the same thing is not something the callers should
/// have to know.
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
    // SAFETY: `dest` is writable by the contract above, and ownership of the
    // duplicate passes through it to the caller.
    unsafe { *dest = dup_string(value) }
}

/// Writes a list into a `GSList **` out-parameter, building it only if the
/// caller wants one — a list built for a NULL out-parameter would have to be
/// freed with the right per-node function, and not building it is simpler than
/// getting that right twice.
///
/// # Safety
///
/// `dest` must be NULL or point at a writable `*mut GSList`, and `build` must
/// yield a list ownership of which may pass to the caller.
pub unsafe fn set_out_list(dest: *mut *mut GSList, build: impl FnOnce() -> *mut GSList) {
    if !dest.is_null() {
        // SAFETY: `dest` is writable by the contract above.
        unsafe { *dest = build() };
    }
}

/// `g_strdup` of a Rust string, which is how a string reaches a list node EDS
/// will free with `g_free`.
///
/// # Safety
///
/// The result is a GLib allocation the caller must `g_free`, or hand to
/// something that will.
pub unsafe fn dup_string(value: &str) -> *mut gchar {
    let text = cstring_lossy(value);
    // SAFETY: `text` is NUL-terminated and valid for the call.
    unsafe { g_strdup(text.as_ptr()) }
}

/// The Rust view of a Camel/GObject instance dispatch already vouches for.
///
/// A class's vfuncs are only ever dispatched by GObject on an instance of
/// that class, so the first argument of `connect_sync`, `get_folder_sync` and
/// their kin needs no type check before the cast — unlike a pointer that
/// arrived via an ordinary property or argument, which does (see the
/// `checked`-family callers instead). This is the trusted half only: a bare
/// null check, then the cast.
///
/// # Safety
///
/// `ptr` must be NULL or point at a live instance of `T`.
pub unsafe fn dispatched_borrow<'a, C, T>(ptr: *mut C) -> Option<&'a T> {
    // SAFETY: the caller's contract is exactly what makes this cast sound.
    unsafe { ptr.cast::<T>().as_ref() }
}

/// The Rust view of a GObject instance that arrived via an ordinary property
/// or argument rather than vfunc dispatch — so, unlike [`dispatched_borrow`],
/// the class dispatch guarantee does not hold and `gtype` must be checked
/// before the cast is sound.
///
/// # Safety
///
/// `ptr` must be NULL or point at a live `GTypeInstance`.
pub unsafe fn checked_borrow<'a, C, T>(ptr: *mut C, gtype: GType) -> Option<&'a T> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: `ptr` is a live `GTypeInstance` by the caller's contract; the
    // type check just performed is what makes the cast below sound.
    unsafe {
        if g_type_check_instance_is_a(ptr.cast(), gtype) == GFALSE {
            return None;
        }
        ptr.cast::<T>().as_ref()
    }
}

/// As [`checked_borrow`], but returns the raw pointer rather than a borrow —
/// for callers where `T` is a foreign C type this crate only forwards to
/// further C calls, not a Rust struct it can safely hand out a reference to.
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

/// Reads `source`'s extension named `name`, without creating it if absent.
///
/// `e_source_get_extension` *creates* the extension it cannot find, which is
/// the wrong answer everywhere a source is only being read — a `.source`
/// keyfile the caller does not own, or an account's own file that must not
/// gain a group merely because something looked at it. This collapses the
/// `e_source_has_extension` guard and the fetch it guards to one call,
/// wherever the pointer needs no further validation than "does the caller's
/// own name find it".
///
/// # Safety
///
/// `source` must be a valid `ESource` that outlives the call, and the
/// extension `name` names, once registered, must be of type `T` — the same
/// contract a bare `.cast::<T>()` on `e_source_get_extension`'s result
/// already carries.
pub unsafe fn extension_if_present<T>(source: *mut ESource, name: &CStr) -> Option<*mut T> {
    // SAFETY: `source` is valid by the caller's contract, and `name` is
    // NUL-terminated by its own type.
    if unsafe { e_source_has_extension(source, name.as_ptr()) } == GFALSE {
        return None;
    }
    // SAFETY: the extension is present, so this returns the source's own,
    // which it owns and which outlives the call, by the caller's contract.
    Some(unsafe { e_source_get_extension(source, name.as_ptr()).cast::<T>() })
}

/// The password EDS fetched from libsecret, if it has one yet.
///
/// An empty stored password is reported as present, not as absent — which is
/// why this is not [`read_string`]. Answering "absent" would make `connect_sync`
/// ask EDS to prompt, and a user who then enters nothing would be prompted
/// again forever. Sending it and being told it is wrong terminates.
///
/// # Safety
///
/// `credentials` must be NULL — which is what EDS passes before it has asked
/// libsecret for anything — or a valid `ENamedParameters`.
pub unsafe fn password(credentials: *const ENamedParameters) -> Option<String> {
    if credentials.is_null() {
        return None;
    }
    // SAFETY: the name is a header constant and the returned string is owned
    // by the parameters, which outlive this call.
    let value = unsafe {
        e_named_parameters_get(credentials, E_SOURCE_CREDENTIAL_PASSWORD.as_ptr()).cast::<gchar>()
    };
    if value.is_null() {
        return None;
    }
    Some(
        // SAFETY: a non-NULL value is a NUL-terminated string owned by the
        // parameters.
        unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned(),
    )
}
