// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The arguments that a signal gives to a C handler, for the one signal in
//! this crate that has parameters.
//!
//! `EMailReader::update-actions` has one `G_TYPE_UINT` parameter and
//! `g_cclosure_marshal_VOID__UINT` (`e-mail-reader.c:5374-5382`,
//! Evolution 3.52.4). `EShellView::update-actions` is `VOID__VOID`
//! (`e-shell-view.c:1090-1097`). The two signals have the same name but
//! different handler signatures, and `jmap-ui` connects to both.
//!
//! Each `g_signal_connect_data` in this crate casts a handler to
//! `unsafe extern "C" fn()` (`jmap_backend_core::callback::gcallback`). C's
//! `G_CALLBACK()` does the same. Thus the compiler cannot check the number of
//! arguments, and only a person who reads the code can check it.
//!
//! This test does the check. It makes a signal with the same shape as the
//! reader signal. It shows that the marshaller calls a handler with three
//! arguments: the instance, the `guint` of the signal, and the user data. A
//! two-argument handler on this signal reads the signal argument as its user
//! data.

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use glib_sys::gpointer;
use gobject_sys::{
    G_TYPE_NONE, G_TYPE_OBJECT, G_TYPE_UINT, GObject, g_cclosure_marshal_VOID__UINT,
    g_object_new_with_properties, g_object_unref, g_signal_connect_data, g_signal_emit_by_name,
    g_signal_new,
};
use jmap_backend_core::callback::gcallback;

static SECOND_ARG: AtomicUsize = AtomicUsize::new(0);
static THIRD_ARG: AtomicUsize = AtomicUsize::new(0);

/// A user-data pointer that is different from all signal arguments.
const USER_DATA: usize = 0xDEAD_BEEF;
/// Represents the `EMailReaderActionGroup` bitmask of the real signal.
const STATE: u32 = 0x2A;

/// The signature that the marshaller calls. The handler of
/// `snooze::browser_ext` must have this signature. It must not have the
/// two-argument signature of the `EShellView` signal, which has no parameters.
unsafe extern "C" fn three_args(_instance: *mut GObject, state: u32, data: gpointer) {
    SECOND_ARG.store(state as usize, Ordering::SeqCst);
    THIRD_ARG.store(data as usize, Ordering::SeqCst);
}

#[test]
fn a_uint_carrying_signal_puts_its_argument_before_the_user_data() {
    // The same shape as "update-actions" of EMailReader, on a plain GObject.
    // Thus the test needs no Evolution instance and no display.
    // SAFETY: a new signal name on G_TYPE_OBJECT, with the marshaller for the
    // one declared parameter.
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

    // SAFETY: no properties, so the count is zero and both arrays are NULL.
    let object =
        unsafe { g_object_new_with_properties(G_TYPE_OBJECT, 0, ptr::null_mut(), ptr::null()) };
    // SAFETY: a live object, the signal registered on its type above, and a
    // handler whose signature agrees with the marshaller of that signal.
    unsafe {
        g_signal_connect_data(
            object,
            c"jmap-probe-update-actions".as_ptr(),
            gcallback(three_args as unsafe extern "C" fn(*mut GObject, u32, gpointer)),
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
