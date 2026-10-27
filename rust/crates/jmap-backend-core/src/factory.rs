// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The GType cell that every backend factory keeps for its backend.
//!
//! A factory registers its backend's `GType` dynamically when EDS loads it as
//! a module, but must still return a valid `GType` when asked before that
//! happens (static linking, or a test). [`BackendTypeCell`] holds the
//! remembered value and falls back to [`register_static`] when none was
//! remembered yet.

use std::marker::PhantomData;
use std::sync::atomic::{AtomicUsize, Ordering};

use glib_sys::GType;

use crate::subclass::{ObjectSubclass, register_static};

/// Stores a backend's `GType`, set once when EDS loads the module.
///
/// [`BackendTypeCell::get`] registers the type statically if nothing was
/// remembered yet, so a factory still works before `e_module_load` runs.
pub struct BackendTypeCell<T> {
    gtype: AtomicUsize,
    _marker: PhantomData<fn() -> T>,
}

impl<T> BackendTypeCell<T> {
    /// Makes a cell with no remembered `GType`.
    pub const fn new() -> Self {
        Self {
            gtype: AtomicUsize::new(0),
            _marker: PhantomData,
        }
    }

    /// Records the dynamically registered backend `GType`.
    pub fn remember(&self, gtype: GType) {
        self.gtype.store(gtype, Ordering::Release);
    }
}

impl<T: ObjectSubclass> BackendTypeCell<T> {
    /// Returns the remembered `GType`, or registers `T` statically as a
    /// fallback when nothing was remembered yet.
    pub fn get(&self) -> GType {
        match self.gtype.load(Ordering::Acquire) {
            0 => register_static::<T>(),
            gtype => gtype,
        }
    }
}

impl<T> Default for BackendTypeCell<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    use glib_sys::GType;
    use gobject_sys::{G_TYPE_OBJECT, GObject, GObjectClass};

    use super::BackendTypeCell;
    use crate::subclass::ObjectSubclass;

    /// An empty type, registered only so [`BackendTypeCell::get`] has a real
    /// `GType` to fall back to.
    struct Stub;

    // SAFETY: GObject and GObjectClass are G_TYPE_OBJECT's own structs, so
    // this subclass adds no fields and needs no extra initialisation.
    unsafe impl ObjectSubclass for Stub {
        const NAME: &'static CStr = c"JmapBackendCoreFactoryStub";
        type Instance = GObject;
        type Class = GObjectClass;

        fn parent_type() -> GType {
            G_TYPE_OBJECT
        }
    }

    /// With nothing remembered, `get` falls back to a real, stable `GType`.
    #[test]
    fn get_falls_back_to_a_registered_type_until_remembered() {
        let cell: BackendTypeCell<Stub> = BackendTypeCell::new();
        let fallback = cell.get();
        assert_ne!(fallback, 0);
        assert_eq!(cell.get(), fallback, "repeated fallback stays stable");

        cell.remember(0xdead_beef);
        assert_eq!(cell.get(), 0xdead_beef, "remember wins over the fallback");
    }

    /// A GObject subclass instance struct holds a raw pointer to its C
    /// struct, so it is not `Sync`. `BackendTypeCell<T>` must still be
    /// `Sync` for book, cal and collection to put it in a `static`.
    #[allow(dead_code)]
    struct HoldsRawPointer(*const ());

    #[test]
    fn cell_is_sync_even_when_t_is_not() {
        static _CELL: BackendTypeCell<HoldsRawPointer> = BackendTypeCell::new();
    }
}
