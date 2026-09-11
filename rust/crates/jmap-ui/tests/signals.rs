// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a signal actually calls a C handler with, for the one signal in this
//! crate whose parameter list is not empty.
//!
//! `EMailReader::update-actions` is declared with one `G_TYPE_UINT` parameter
//! and `g_cclosure_marshal_VOID__UINT` (`e-mail-reader.c:5374-5382`,
//! Evolution 3.52.4), unlike `EShellView::update-actions`, which is
//! `VOID__VOID` (`e-shell-view.c:1090-1097`). Two signals of the same name,
//! two different handler signatures — and `jmap-ui` connects to both.
//!
//! Every `g_signal_connect_data` in this crate transmutes a handler to
//! `unsafe extern "C" fn()`, which is what C does too and what makes the
//! arity a thing only a reader can check. This test is the check: it builds a
//! signal shaped exactly like the reader's and shows the marshaller calling a
//! handler with three arguments — instance, the signal's own `guint`, then
//! the user data — so a two-argument handler connected to it would be reading
//! the signal's argument where it believes its user data is.

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use glib_sys::gpointer;
use gobject_sys::{
    G_TYPE_NONE, G_TYPE_OBJECT, G_TYPE_UINT, GObject, g_cclosure_marshal_VOID__UINT,
    g_object_new_with_properties, g_object_unref, g_signal_connect_data, g_signal_emit_by_name,
    g_signal_new,
};

static SECOND_ARG: AtomicUsize = AtomicUsize::new(0);
static THIRD_ARG: AtomicUsize = AtomicUsize::new(0);

/// A user-data pointer no signal argument could be mistaken for.
const USER_DATA: usize = 0xDEAD_BEEF;
/// Stands for the `EMailReaderActionGroup` bitmask the real signal carries.
const STATE: u32 = 0x2A;

/// The signature the marshaller really calls: the handler
/// `snooze::browser_ext` installs has to match this one, not the two-argument
/// shape `EShellView`'s parameterless signal takes.
unsafe extern "C" fn three_args(_instance: *mut GObject, state: u32, data: gpointer) {
    SECOND_ARG.store(state as usize, Ordering::SeqCst);
    THIRD_ARG.store(data as usize, Ordering::SeqCst);
}

#[test]
fn a_uint_carrying_signal_puts_its_argument_before_the_user_data() {
    // Shaped exactly like EMailReader's "update-actions", on a plain GObject
    // so the test needs no Evolution instance (and so no display).
    // SAFETY: a fresh signal name on G_TYPE_OBJECT, with the marshaller that
    // matches the one parameter declared.
    let id = unsafe {
        g_signal_new(
            c"jmap-probe-update-actions".as_ptr(),
            G_TYPE_OBJECT,
            gobject_sys::G_SIGNAL_RUN_FIRST,
            0,
            None,
            ptr::null_mut(),
            Some(g_cclosure_marshal_VOID__UINT),
            G_TYPE_NONE,
            1,
            G_TYPE_UINT,
        )
    };
    assert_ne!(id, 0, "the probe signal was not created");

    // SAFETY: no properties, so the count is zero and both arrays NULL.
    let object =
        unsafe { g_object_new_with_properties(G_TYPE_OBJECT, 0, ptr::null_mut(), ptr::null()) };
    // SAFETY: a live object, the signal just registered on its type, and a
    // handler whose signature matches that signal's marshaller.
    unsafe {
        g_signal_connect_data(
            object,
            c"jmap-probe-update-actions".as_ptr(),
            Some(std::mem::transmute::<
                unsafe extern "C" fn(*mut GObject, u32, gpointer),
                unsafe extern "C" fn(),
            >(three_args)),
            USER_DATA as *mut c_void,
            None,
            0,
        );
        g_signal_emit_by_name(object, c"jmap-probe-update-actions".as_ptr(), STATE);
        g_object_unref(object);
    }

    assert_eq!(
        SECOND_ARG.load(Ordering::SeqCst),
        STATE as usize,
        "the signal's own argument is the handler's second, not its user data"
    );
    assert_eq!(
        THIRD_ARG.load(Ordering::SeqCst),
        USER_DATA,
        "the user data arrives third"
    );
}
