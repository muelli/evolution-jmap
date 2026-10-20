// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pointers held for the rest of the process.
//!
//! Some C objects are created once and must never be released. Examples are
//! a `CamelProvider` that Camel keeps in a table it never clears, and the
//! `EOAuth2Services` whose last reference must not drop while another thread
//! queries it. The natural location for one is a `static OnceLock`, which
//! needs `Send + Sync`. A raw pointer is neither. [`Forever`] is that pointer
//! with both, so each site does not write its own wrapper struct and
//! `unsafe impl` pair.
//!
//! The obligation that made each hand-written copy sound moves to
//! [`Forever::new`]. The caller tells why it is safe to share this pointer
//! between threads, one time, where it creates the pointer.

/// A raw pointer that is never released, shareable between threads.
///
/// There is intentionally no `Drop`: at no point in the life of the process
/// is it better to release the pointer.
pub struct Forever<T>(*mut T);

// SAFETY: `Forever::new`'s contract: the pointee lives for the rest of the
// process. Every use of the pointer from any thread is sound without
// synchronisation by this holder. Nothing writes the pointer itself after
// construction.
unsafe impl<T> Send for Forever<T> {}
// SAFETY: as `Send`.
unsafe impl<T> Sync for Forever<T> {}

impl<T> Forever<T> {
    /// Holds `ptr` for the rest of the process.
    ///
    /// # Safety
    ///
    /// `ptr` must be NULL or point at something that is never freed or moved
    /// for the rest of the process. Every use of it from any thread must be
    /// sound without more synchronisation. Thus, either the pointee is written
    /// only before it is published (as a `OnceLock` initialiser does), or it
    /// guards its own state (as a GObject with an internal lock does).
    pub const unsafe fn new(ptr: *mut T) -> Self {
        Self(ptr)
    }

    /// The held pointer.
    pub const fn as_ptr(&self) -> *mut T {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use super::Forever;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn a_static_once_lock_can_hold_one() {
        assert_send_sync::<Forever<u8>>();
        static HELD: OnceLock<Forever<u8>> = OnceLock::new();
        let target = Box::into_raw(Box::new(7_u8));
        // SAFETY: leaked above, and only read below.
        let held = HELD.get_or_init(|| unsafe { Forever::new(target) });
        assert_eq!(held.as_ptr(), target);
        // SAFETY: the leaked box is live and nothing writes to it.
        assert_eq!(unsafe { *held.as_ptr() }, 7);
    }
}
