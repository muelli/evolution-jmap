// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Each EDS vfunc receives a GCancellable, but the JMAP client only knows its
// own CancelFlag. The bridge sends a Stop from the user to an HTTP request
// that is in progress.

use gio_sys::{g_cancellable_cancel, g_cancellable_new};
use gobject_sys::g_object_unref;
use jmap_backend_core::cancel::{CancelBridge, observe};
use jmap_client::transport::{CancelFlag, CancelScope, observed};
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

/// EDS gives an already-cancelled cancellable when a request aborts before the
/// backend starts it. Then `g_cancellable_connect` invokes the callback
/// immediately. The bridge must use this and not start the operation.
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

/// A vfunc gives its cancellable to [`observe`] and keeps the result. Thus each
/// request of the operation checks the cancellable. This is also true for a
/// client built long before the operation, in a layer that never sees a
/// `GCancellable`.
#[test]
fn an_observed_cancellable_is_what_this_thread_is_cancelled_through() {
    let cancellable = unsafe { g_cancellable_new() };
    let operation = unsafe { observe(cancellable) };

    let flag = observed().expect("the operation installed a scope");
    assert!(!flag.is_cancelled());
    unsafe { g_cancellable_cancel(cancellable) };
    assert!(
        observed().expect("still installed").is_cancelled(),
        "the Stop the user pressed did not reach the thread's scope"
    );

    drop(operation);
    assert!(
        observed().is_none(),
        "the operation left its cancellable installed after it returned"
    );
    unsafe { g_object_unref(cancellable.cast()) };
}

/// A NULL cancellable means "this call cannot be cancelled". A flag that never
/// fires would *mask* the outer operation of this nested call. A folder vfunc
/// that calls its store passes NULL when it has no cancellable of its own.
/// Thus NULL installs nothing.
#[test]
fn a_null_cancellable_leaves_the_operation_around_it_observed() {
    let outer = CancelFlag::new();
    let _outer = CancelScope::install(&outer);

    {
        let _inner = unsafe { observe(ptr::null_mut()) };
        outer.cancel();
        assert!(
            observed()
                .expect("the outer operation is still what is observed")
                .is_cancelled(),
            "an uncancellable inner call hid the cancellation of the one around it"
        );
    }

    assert!(observed().expect("the outer scope survived").is_cancelled());
}

/// A drop of the bridge disconnects the GIO handler. A later cancellation
/// must not reach a flag cloned before the drop. This proves that the handler
/// is gone and not only leaked.
#[test]
fn dropping_the_bridge_disconnects_the_handler() {
    let cancellable = unsafe { g_cancellable_new() };
    let bridge = unsafe { CancelBridge::new(cancellable) };
    let flag = bridge.flag().clone();

    drop(bridge);
    unsafe { g_cancellable_cancel(cancellable) };
    assert!(
        !flag.is_cancelled(),
        "the handler fired after the bridge that connected it was dropped"
    );

    unsafe { g_object_unref(cancellable.cast()) };
}

/// The client receives the flag by value. Thus a clone of the flag must
/// continue to work and point to the same shared state.
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
