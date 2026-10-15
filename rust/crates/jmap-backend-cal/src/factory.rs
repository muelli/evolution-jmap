// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `ECalBackendFactory` subclass mapping `BackendName=jmap` to [`JmapCalBackend`].

use std::ffi::CStr;
use std::sync::atomic::{AtomicUsize, Ordering};

use eds_sys::{
    ECalBackendFactory, ECalBackendFactoryClass, I_CAL_VEVENT_COMPONENT, ICalComponentKind,
    e_cal_backend_factory_get_type,
};
use glib_sys::GType;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};

use crate::backend::JmapCalBackend;

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
static BACKEND_TYPE: AtomicUsize = AtomicUsize::new(0);

/// Records the dynamically registered backend `GType` for factory instantiation.
pub fn remember_backend_type(gtype: GType) {
    BACKEND_TYPE.store(gtype, Ordering::Release);
}

/// Returns the backend GType, registering statically as fallback if unloaded.
fn backend_type() -> GType {
    match BACKEND_TYPE.load(Ordering::Acquire) {
        0 => register_static::<JmapCalBackend>(),
        gtype => gtype,
    }
}

// SAFETY: both structs are #[repr(C)] and lead with the ECalBackendFactory
// instance and class structs respectively, and ECalBackendFactory derives from
// GObject (via EBackendFactory and EExtension).
unsafe impl ObjectSubclass for JmapCalFactory {
    const NAME: &'static CStr = c"ECalBackendJmapFactory";
    type Instance = JmapCalFactory;
    type Class = JmapCalFactoryClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the EDS type system initialises itself.
        unsafe { e_cal_backend_factory_get_type() }
    }

    fn class_init_types() -> Vec<GType> {
        // Pre-register backend type before class init to avoid lock inversion.
        vec![backend_type()]
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points at a freshly allocated class struct of ours,
        // which leads with the parent's; all three fields are in that half.
        let factory = unsafe { &mut (*class).parent_class };
        factory.factory_name = FACTORY_NAME.as_ptr();
        factory.component_kind = COMPONENT_KIND;
        factory.backend_type = backend_type();
    }
}
