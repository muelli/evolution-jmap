// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Handlers as `GCallback`s.
//!
//! `g_signal_connect_data`, `g_signal_connect_object` and
//! `g_cancellable_connect` take a `GCallback`, a `void (*)(void)` that C's
//! `G_CALLBACK()` casts any handler to and the signal's marshaller casts back
//! to the arity the signal declares. Rust has no implicit cast between
//! function pointer types, so every connection used to spell that cast as a
//! hand-written `transmute`. [`gcallback`] is the one place that does it now.
//!
//! Its argument must be a C function pointer: the conversion is implemented
//! for `unsafe extern "C" fn` types only, so a closure, a Rust-ABI function
//! or a data pointer does not compile. A function item has to be coerced
//! first (`on_changed as unsafe extern "C" fn(*mut GObject, gpointer)`),
//! which keeps the signature visible next to the signal it is connected to.
//!
//! Erasing is safe: it only changes the pointer's type. What stays the
//! caller's obligation, at the `unsafe` connect call, is that the handler's
//! real signature is the one the signal invokes it with. GLib cannot check
//! that, and a mismatch is undefined behaviour when the signal fires.

/// A C function pointer that [`gcallback`] can erase.
///
/// Sealed: implemented for `unsafe extern "C" fn` types of up to six
/// arguments, which covers every signal this project connects to.
pub trait CHandler: sealed::Sealed + Copy {
    /// The same address, typed as GLib's erased `void (*)(void)`.
    fn erase(self) -> unsafe extern "C" fn();
}

mod sealed {
    pub trait Sealed {}
}

macro_rules! c_handler {
    ($($arg:ident),*) => {
        impl<R, $($arg),*> sealed::Sealed for unsafe extern "C" fn($($arg),*) -> R {}

        impl<R, $($arg),*> CHandler for unsafe extern "C" fn($($arg),*) -> R {
            fn erase(self) -> unsafe extern "C" fn() {
                // SAFETY: both types are C function pointers, which have the
                // same size and representation whatever their signature, so
                // this only retypes the address. Calling the result needs
                // `unsafe`, and is the connect call's obligation (above).
                unsafe { std::mem::transmute::<Self, unsafe extern "C" fn()>(self) }
            }
        }
    };
}

c_handler!();
c_handler!(A);
c_handler!(A, B);
c_handler!(A, B, C);
c_handler!(A, B, C, D);
c_handler!(A, B, C, D, E);
c_handler!(A, B, C, D, E, F);

/// `handler` as the `GCallback` a GLib connect function takes: `G_CALLBACK()`.
pub fn gcallback(handler: impl CHandler) -> Option<unsafe extern "C" fn()> {
    Some(handler.erase())
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use gio_sys::{GCancellable, g_cancellable_cancel, g_cancellable_new};
    use glib_sys::gpointer;
    use gobject_sys::{g_object_unref, g_signal_connect_data};

    use super::gcallback;

    unsafe extern "C" fn two_args(_: *mut c_void, _: u32) {}

    #[test]
    fn erasing_a_handler_keeps_its_address() {
        let handler = two_args as unsafe extern "C" fn(*mut c_void, u32);
        let erased = gcallback(handler).expect("an erased handler is never NULL");
        assert_eq!(erased as usize, handler as usize);
    }

    static SEEN: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn on_cancelled(cancellable: *mut GCancellable, data: gpointer) {
        assert!(!cancellable.is_null(), "the emitter comes first");
        SEEN.store(data as usize, Ordering::SeqCst);
    }

    #[test]
    fn glib_calls_an_erased_handler_with_the_signals_own_arguments() {
        const DATA: usize = 0x5ca1ab1e;
        // SAFETY: a fresh cancellable, a handler with the signature
        // "cancelled" is emitted with, and user data nothing dereferences.
        unsafe {
            let cancellable = g_cancellable_new();
            let id = g_signal_connect_data(
                cancellable.cast(),
                c"cancelled".as_ptr(),
                gcallback(on_cancelled as unsafe extern "C" fn(*mut GCancellable, gpointer)),
                DATA as gpointer,
                None,
                0,
            );
            assert_ne!(id, 0, "nothing connected to \"cancelled\"");
            g_cancellable_cancel(cancellable);
            g_object_unref(cancellable.cast());
        }
        assert_eq!(SEEN.load(Ordering::SeqCst), DATA);
    }
}
