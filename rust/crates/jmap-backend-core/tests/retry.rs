// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This binary holds one test only. The test registers the EDS client error
// domain as a GLib extended error domain, so that each `g_error_free` of
// that domain is counted. GLib frees an error with the private size of its
// domain at free time. Thus no error of the domain may exist before the
// registration, and no other test may share this process.

use std::cell::Cell;
use std::ffi::c_char;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use eds_sys::e_client_error_quark;
use glib_sys::{GError, GErrorClearFunc, GErrorCopyFunc, GErrorInitFunc, GQuark, GTRUE, gboolean};
use jmap_backend_core::error::{set_raw_gerror, to_gerror};
use jmap_backend_core::retry::retry_on_authentication_failure;
use jmap_client::Error;

static FREED: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn init(_: *mut GError) {}

unsafe extern "C" fn copy(_: *const GError, _: *mut GError) {}

unsafe extern "C" fn clear(_: *mut GError) {
    FREED.fetch_add(1, Ordering::SeqCst);
}

// The workspace builds glib-sys without its `v2_68` feature, so this test
// declares the function itself.
unsafe extern "C" {
    fn g_error_domain_register_static(
        error_type_name: *const c_char,
        error_type_private_size: usize,
        error_type_init: GErrorInitFunc,
        error_type_copy: GErrorCopyFunc,
        error_type_clear: GErrorClearFunc,
    ) -> GQuark;
}

#[test]
fn the_spent_401_is_freed_and_its_slot_is_empty_before_the_retry() {
    // SAFETY: the quark name is a static string. No error of the domain
    // exists yet, and the callbacks touch no memory of the error.
    unsafe {
        let name = glib_sys::g_quark_to_string(e_client_error_quark());
        let quark = g_error_domain_register_static(
            name,
            size_of::<u64>(),
            Some(init),
            Some(copy),
            Some(clear),
        );
        assert_eq!(quark, e_client_error_quark());
    }

    let attempts = Cell::new(0);
    let mut reported: *mut GError = ptr::null_mut();

    // SAFETY: `reported` is a writable, currently-NULL `GError *`.
    let result = unsafe {
        retry_on_authentication_failure(
            &mut reported,
            |dest| -> gboolean {
                attempts.set(attempts.get() + 1);
                assert!(
                    (*dest).is_null(),
                    "attempt {} got a used slot",
                    attempts.get()
                );
                if attempts.get() == 1 {
                    let unauthorized = Error::Http {
                        status: 401,
                        problem: None,
                    };
                    set_raw_gerror(dest, to_gerror(&unauthorized));
                    glib_sys::GFALSE
                } else {
                    assert_eq!(FREED.load(Ordering::SeqCst), 1, "the 401 is not freed");
                    GTRUE
                }
            },
            || true,
        )
    };

    assert_eq!(result, GTRUE);
    assert_eq!(attempts.get(), 2);
    assert!(reported.is_null());
    assert_eq!(FREED.load(Ordering::SeqCst), 1);
}
