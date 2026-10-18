// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pointers held for the rest of the process.
//!
//! Some C objects are created once and must never be released: a
//! `CamelProvider` Camel keeps in a table it never clears, the
//! `EOAuth2Services` whose last reference must not drop while another thread
//! queries it. The natural home for one is a `static OnceLock`, which needs
//! `Send + Sync`, and a raw pointer is neither. [`Forever`] is that pointer
//! with both, so each site no longer hand-writes its own wrapper struct and
//! `unsafe impl` pair.
//!
//! The obligation that made each hand copy sound moves to
//! [`Forever::new`]: the caller says why sharing this pointer between threads
//! is safe, once, where it is created.

/// A raw pointer that is never released, shareable between threads.
///
/// No `Drop`, deliberately: there is no point in the process's life at which
/// releasing it would be an improvement.
pub struct Forever<T>(*mut T);

// SAFETY: `Forever::new`'s contract: the pointee lives for the rest of the
// process and every use of the pointer from any thread is sound without this
// holder synchronising anything. The pointer itself is never written after
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
    /// for the rest of the process, and every use any thread makes of it must
    /// be sound without further synchronisation: the pointee is written only
    /// before it is published (as a `OnceLock` initialiser does), or it guards
    /// its own state, as a GObject with an internal lock does.
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
