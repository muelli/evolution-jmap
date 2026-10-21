// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The parent classes are initialised before the first test starts.
//!
//! This binary has one test, so no other test can initialise them first. The
//! test looks up the types by name, because an accessor registers its type.

use std::ffi::CStr;

use gobject_sys::{g_type_class_peek, g_type_from_name};
// The crate is linked only when a test names it. The other tests use its items.
use jmap_backend_cal as _;

fn class_is_initialised(name: &CStr) -> bool {
    // SAFETY: a NUL-terminated name. A peek takes no reference.
    unsafe {
        let gtype = g_type_from_name(name.as_ptr());
        gtype != 0 && !g_type_class_peek(gtype).is_null()
    }
}

/// `EBackend`'s `class_init` registers `ESource`. If a test thread does that
/// while a different one registers `ESource`, both threads stop.
#[test]
fn the_backend_and_factory_parents_are_ready_before_any_test_runs() {
    assert!(class_is_initialised(c"ECalMetaBackend"));
    assert!(class_is_initialised(c"ECalBackendFactory"));
    // SAFETY: as above.
    assert_ne!(unsafe { g_type_from_name(c"ESource".as_ptr()) }, 0);
}
