// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelJmapStore`: the type Camel instantiates for a JMAP mail account.
//!
//! No vfunc is overridden yet. What the type does carry is the two things a
//! later increment cannot change cheaply: its parent, because every folder
//! vfunc is declared on one of the two candidates, and the settings class it
//! is configured through — [`crate::settings`], without which a JMAP account
//! has nowhere to keep a server.

use std::ffi::CStr;

use eds_sys::{
    CamelOfflineStore, CamelOfflineStoreClass, CamelServiceClass, camel_offline_store_get_type,
};
use glib_sys::GType;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};

use crate::settings::settings_type;

/// The instance struct. `#[repr(C)]` leading with the parent's instance struct
/// is what makes a `*mut JmapStore` usable as the `CamelStore *` every Camel
/// function takes.
#[repr(C)]
pub struct JmapStore {
    parent: CamelOfflineStore,
}

/// The class struct, same rule one level up. It will grow overrides of
/// `CamelStoreClass`'s folder vfuncs; today it adds nothing of its own, which
/// is still not the same as *being* `CamelOfflineStoreClass` — the type needs
/// its own class for the overrides to have somewhere to go.
#[repr(C)]
pub struct JmapStoreClass {
    parent_class: CamelOfflineStoreClass,
}

// SAFETY: both structs are #[repr(C)] and lead with the CamelOfflineStore
// instance and class structs, whose layouts eds-sys's tests/layout.rs checks
// against `g_type_query`; CamelOfflineStore derives from CamelStore, from
// CamelService, from GObject.
unsafe impl ObjectSubclass for JmapStore {
    /// `CamelJmapStore`, not `JmapStore`: Camel's own stores are all
    /// `Camel<Protocol>Store`, and the type name is what a user sees in a
    /// GObject warning about the wrong store type.
    const NAME: &'static CStr = c"CamelJmapStore";
    type Instance = JmapStore;
    type Class = JmapStoreClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { camel_offline_store_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // Which class `camel_service_ref_settings` instantiates when nothing
        // has handed the service a settings object — and, more to the point,
        // which class Evolution's account editor and
        // `e_source_camel_configure_service` expect to configure. Inherited it
        // would be `CamelOfflineSettings`, which carries no server at all, so
        // this one line is what connects a JMAP account to a host.
        //
        // SAFETY: the class leads with CamelOfflineStoreClass, which derives
        // from CamelStoreClass, from CamelServiceClass — the contract above.
        unsafe { (*class.cast::<CamelServiceClass>()).settings_type = settings_type() };
    }
}

/// Registers the store type, or returns it if it is already registered.
///
/// Statically, unlike the EDS backends' types: a Camel provider is not a
/// `GTypeModule`. Camel dlopens the module and never closes it, and the
/// provider struct it keeps a pointer to names these `GType`s forever, so there
/// is no unload for a dynamic type to be unregistered by — and a type that
/// *could* be unloaded here would be one Camel could still be asked to
/// instantiate.
pub fn store_type() -> GType {
    register_static::<JmapStore>()
}
