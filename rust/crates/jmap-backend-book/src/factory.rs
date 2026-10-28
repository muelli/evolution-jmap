// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `EBookBackendFactory` subclass mapping `BackendName=jmap` to [`JmapBookBackend`].

use std::ffi::CStr;

use eds_sys::{EBookBackendFactory, EBookBackendFactoryClass, e_book_backend_factory_get_type};
use glib_sys::GType;
use jmap_backend_core::subclass::ObjectSubclass;

use crate::backend::JmapBookBackend;

pub use jmap_backend_core::factory::BackendTypeCell;

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
pub static BACKEND_TYPE: BackendTypeCell<JmapBookBackend> = BackendTypeCell::new();

/// Records the dynamically registered backend `GType` for factory instantiation.
pub fn remember_backend_type(gtype: GType) {
    BACKEND_TYPE.remember(gtype);
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
        vec![BACKEND_TYPE.get()]
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points to a freshly allocated class struct; both fields are in the parent struct.
        let factory = unsafe { &mut (*class).parent_class };
        factory.factory_name = FACTORY_NAME.as_ptr();
        factory.backend_type = BACKEND_TYPE.get();
    }
}
