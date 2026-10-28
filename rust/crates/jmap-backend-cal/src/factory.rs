// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `ECalBackendFactory` subclass mapping `BackendName=jmap` to [`JmapCalBackend`].

use std::ffi::CStr;

use eds_sys::{
    ECalBackendFactory, ECalBackendFactoryClass, I_CAL_VEVENT_COMPONENT, ICalComponentKind,
    e_cal_backend_factory_get_type,
};
use glib_sys::GType;
use jmap_backend_core::subclass::ObjectSubclass;

use crate::backend::JmapCalBackend;

pub use jmap_backend_core::factory::BackendTypeCell;

/// Backend name matched against `BackendName` in calendar source files.
pub const FACTORY_NAME: &CStr = c"jmap";

/// Component kind served by this factory (VEVENT).
pub const COMPONENT_KIND: ICalComponentKind = I_CAL_VEVENT_COMPONENT;

/// The factory EDS asks for a [`JmapCalBackend`].
#[repr(C)]
pub struct JmapCalFactory {
    parent: ECalBackendFactory,
}

/// Class struct for [`JmapCalFactory`].
#[repr(C)]
pub struct JmapCalFactoryClass {
    pub parent_class: ECalBackendFactoryClass,
}

/// Dynamically registered GType for `JmapCalBackend`.
pub static BACKEND_TYPE: BackendTypeCell<JmapCalBackend> = BackendTypeCell::new();

/// Records the dynamically registered backend `GType` for factory instantiation.
pub fn remember_backend_type(gtype: GType) {
    BACKEND_TYPE.remember(gtype);
}

// SAFETY: Both structs are #[repr(C)] and begin with ECalBackendFactory instance
// and class structs. ECalBackendFactory derives from GObject.
unsafe impl ObjectSubclass for JmapCalFactory {
    const NAME: &'static CStr = c"ECalBackendJmapFactory";
    type Instance = JmapCalFactory;
    type Class = JmapCalFactoryClass;

    fn parent_type() -> GType {
        // SAFETY: The function takes no arguments and EDS initializes its type system.
        unsafe { e_cal_backend_factory_get_type() }
    }

    fn class_init_types() -> Vec<GType> {
        // Register backend type before class init to avoid lock inversion.
        vec![BACKEND_TYPE.get()]
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points to an allocated class struct containing parent fields.
        let factory = unsafe { &mut (*class).parent_class };
        factory.factory_name = FACTORY_NAME.as_ptr();
        factory.component_kind = COMPONENT_KIND;
        factory.backend_type = BACKEND_TYPE.get();
    }
}
