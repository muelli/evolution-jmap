// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Every EDS vfunc receives a GCancellable; the JMAP client only understands
// its own CancelFlag. The bridge is what makes "the user hit Stop" reach an
// in-flight HTTP request.

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

/// What a vfunc actually does with its cancellable: hand it to [`observe`] and
/// hold the result, so that every request the operation makes — through a client
/// built long before it, by a layer that never sees a `GCancellable` — is
/// checked against it.
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

/// A NULL cancellable means "this call cannot be cancelled", and installing a
/// flag that can never fire for it would *mask* the operation this one is
/// nested inside — a folder vfunc that calls into its store passes NULL where
/// it has nothing of its own to pass. So NULL installs nothing.
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
