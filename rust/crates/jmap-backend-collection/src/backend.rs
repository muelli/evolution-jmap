// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `ECollectionBackend` subclass itself.
//!
//! Deliberately dull, like `jmap-backend-book`'s: the instance and class
//! structs, the vfunc slots, and a panic guard in front of each body — the
//! bodies themselves live a layer down, where they can be tested without a
//! GObject. What is different here is that the slots are not empty to begin
//! with. `ECollectionBackendClass` installs a working `dup_resource_id` and a
//! do-nothing `populate`, so an override that is written but not *installed*
//! does not produce a backend that fails; it produces one that quietly answers
//! something else. `tests/backend.rs` holds each slot against the parent's to
//! keep that from being invisible.

use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::ptr;

use eds_sys::{
    ECollectionBackend, ECollectionBackendClass, ESource, e_collection_backend_get_type,
};
use glib_sys::{GType, gchar};
use jmap_backend_core::marshal::dup_string;
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_backend_core::trampoline::guard;

use crate::resource_id::resource_id_of;

/// The JMAP collection backend.
#[repr(C)]
pub struct JmapCollectionBackend {
    /// GObject's; never read by this code, only handed back to EDS as the
    /// instance pointer it gave us.
    parent: ECollectionBackend,
}

/// The class struct. Nothing of ours lives in it; it exists because GObject
/// needs a size to allocate and a place to put the vfunc slots.
#[repr(C)]
pub struct JmapCollectionBackendClass {
    pub parent_class: ECollectionBackendClass,
}

impl JmapCollectionBackend {
    /// An instance outside the GObject type system: zeroed parent bytes, which
    /// is what `instance_init` leaves behind minus the GObject.
    ///
    /// As in `jmap-backend-book`, this exists because a real instance needs an
    /// `ESourceRegistryServer` and so a running `evolution-source-registry` on
    /// the session bus, which neither the test VM nor CI has. Nothing may be
    /// touched through the result: the parent bytes are a valid bit pattern but
    /// they are not a GObject, so passing one to any EDS function is undefined
    /// behaviour. The one vfunc below never reads the backend at all, which is
    /// what makes driving it this way sound rather than merely convenient.
    pub fn detached() -> Box<Self> {
        // SAFETY: every field of the parent is a pointer or an integer, for
        // which all-zero is a valid value.
        Box::new(unsafe { MaybeUninit::zeroed().assume_init() })
    }
}

// SAFETY: both structs are #[repr(C)] and lead with the ECollectionBackend
// instance and class structs respectively, and ECollectionBackend derives from
// GObject (via EBackend).
unsafe impl ObjectSubclass for JmapCollectionBackend {
    const NAME: &'static CStr = c"ECollectionBackendJmap";
    type Instance = JmapCollectionBackend;
    type Class = JmapCollectionBackendClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the EDS type system initialises itself.
        unsafe { e_collection_backend_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points at a freshly allocated class struct of ours,
        // which leads with the parent's; the slot below is in that half.
        let vfuncs = unsafe { &mut (*class).parent_class };
        vfuncs.dup_resource_id = Some(dup_resource_id);
    }
}

/// What EDS calls a cached child source to learn which collection it stands
/// for, once per file, before `populate`.
///
/// The `backend` argument is not read: the answer is a function of the child
/// source alone, which is what makes it testable without an instance — and what
/// makes it safe to drive from a detached one.
///
/// A panic becomes NULL, and NULL here means EDS deletes the file. That is the
/// wrong outcome, but there is no other: the vfunc has no `GError` and no
/// sentinel that means "ask me again". The guard logs a critical, which is the
/// only trace such a bug can leave.
unsafe extern "C" fn dup_resource_id(
    backend: *mut ECollectionBackend,
    child_source: *mut ESource,
) -> *mut gchar {
    let _ = backend;
    guard("dup_resource_id", ptr::null_mut(), || {
        // SAFETY: EDS hands us one of its own sources, alive for the call.
        match unsafe { resource_id_of(child_source) } {
            // SAFETY: ownership of the duplicate passes to EDS, which puts it
            // in its hash table and frees it with `g_free`.
            Some(id) => unsafe { dup_string(&id) },
            None => ptr::null_mut(),
        }
    })
}
