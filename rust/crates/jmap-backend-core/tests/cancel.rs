// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Every EDS vfunc receives a GCancellable; the JMAP client only understands
// its own CancelFlag. The bridge is what makes "the user hit Stop" reach an
// in-flight HTTP request.

use gio_sys::{g_cancellable_cancel, g_cancellable_new};
use gobject_sys::g_object_unref;
use jmap_backend_core::cancel::CancelBridge;
use std::ptr;

#[test]
fn cancelling_the_gcancellable_raises_the_flag() {
    let cancellable = unsafe { g_cancellable_new() };
    let bridge = unsafe { CancelBridge::new(cancellable) };

    assert!(!bridge.flag().is_cancelled());
    unsafe { g_cancellable_cancel(cancellable) };
    assert!(bridge.flag().is_cancelled());

    drop(bridge);
    unsafe { g_object_unref(cancellable.cast()) };
}

/// EDS hands out already-cancelled cancellables when a request is aborted
/// before the backend gets to it. `g_cancellable_connect` invokes the callback
/// immediately in that case; make sure we rely on that rather than starting
/// the operation regardless.
#[test]
fn a_cancellable_that_is_already_cancelled_starts_out_raised() {
    let cancellable = unsafe { g_cancellable_new() };
    unsafe { g_cancellable_cancel(cancellable) };

    let bridge = unsafe { CancelBridge::new(cancellable) };
    assert!(bridge.flag().is_cancelled());

    drop(bridge);
    unsafe { g_object_unref(cancellable.cast()) };
}

/// A NULL GCancellable is legal everywhere in GIO and means "uncancellable".
#[test]
fn a_null_cancellable_yields_a_flag_that_never_fires() {
    let bridge = unsafe { CancelBridge::new(ptr::null_mut()) };
    assert!(!bridge.flag().is_cancelled());
}

/// The flag is handed to the client by value, so it has to keep working (and
/// keep pointing at the same shared state) after being cloned out.
#[test]
fn the_flag_can_be_cloned_out_and_still_observes_cancellation() {
    let cancellable = unsafe { g_cancellable_new() };
    let bridge = unsafe { CancelBridge::new(cancellable) };
    let flag = bridge.flag().clone();

    unsafe { g_cancellable_cancel(cancellable) };
    assert!(flag.is_cancelled());

    drop(bridge);
    unsafe { g_object_unref(cancellable.cast()) };
}
