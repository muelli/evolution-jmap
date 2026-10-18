// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `EBookBackendFactory` subclass mapping `BackendName=jmap` to [`JmapBookBackend`].

use std::ffi::CStr;
use std::sync::atomic::{AtomicUsize, Ordering};

use eds_sys::{EBookBackendFactory, EBookBackendFactoryClass, e_book_backend_factory_get_type};
use glib_sys::GType;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};

use crate::backend::JmapBookBackend;

/// Backend name matched against `BackendName` in address book source files.
pub const FACTORY_NAME: &CStr = c"jmap";

/// Factory for [`JmapBookBackend`].
#[repr(C)]
pub struct JmapBookFactory {
    /// Base class retains the instance state.
    parent: EBookBackendFactory,
}

/// Class struct for [`JmapBookFactory`].
#[repr(C)]
pub struct JmapBookFactoryClass {
    pub parent_class: EBookBackendFactoryClass,
}

/// Dynamically registered GType for `JmapBookBackend`.
static BACKEND_TYPE: AtomicUsize = AtomicUsize::new(0);

/// Records the dynamically registered backend `GType` for factory instantiation.
pub fn remember_backend_type(gtype: GType) {
    BACKEND_TYPE.store(gtype, Ordering::Release);
}

/// Returns the backend GType, registering statically as fallback if unloaded.
fn backend_type() -> GType {
    match BACKEND_TYPE.load(Ordering::Acquire) {
        0 => register_static::<JmapBookBackend>(),
        gtype => gtype,
    }
}

// SAFETY: Structs are #[repr(C)] and lead with EBookBackendFactory structs. EBookBackendFactory derives from GObject.
unsafe impl ObjectSubclass for JmapBookFactory {
    const NAME: &'static CStr = c"EBookBackendJmapFactory";
    type Instance = JmapBookFactory;
    type Class = JmapBookFactoryClass;

    fn parent_type() -> GType {
        // SAFETY: No arguments; type initialises itself.
        unsafe { e_book_backend_factory_get_type() }
    }

    fn class_init_types() -> Vec<GType> {
        // Pre-register backend type before class init to avoid lock inversion.
        vec![backend_type()]
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points to a freshly allocated class struct; both fields are in the parent struct.
        let factory = unsafe { &mut (*class).parent_class };
        factory.factory_name = FACTORY_NAME.as_ptr();
        factory.backend_type = backend_type();
    }
}
