// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `ECollectionBackendFactory` subclass: how `evolution-source-registry`
//! goes from the `.source` file of an account to a [`JmapCollectionBackend`].
//!
//! The registry server is an `EDataFactory` and also an `EExtensible`. These
//! two roles are the full mechanism. As an extensible, it makes one instance of
//! each registered `EExtension` whose `extensible_type` is
//! `E_TYPE_SOURCE_REGISTRY_SERVER`. Each `ECollectionBackendFactory` inherits
//! this type, so this file registers nothing with the server. As a data
//! factory, it stores those extensions by the key of each one. It gives an
//! `ESource` to the extension whose key is the `BackendName` in the
//! `[Collection]` group of the source.
//!
//! Thus this file makes the same two assignments as the factory of
//! `jmap-backend-book`. The difference is the result when they are missing.
//! `e_collection_backend_factory_class_init` sets both fields to working
//! values: `factory_name` is `"none"` and `backend_type` is
//! `E_TYPE_COLLECTION_BACKEND`. Thus an incomplete factory gives no error. Its
//! name matches no account, or it builds the empty collection backend of EDS.
//! The user then gets an account that connects to nothing and shows no error.
//! `tests/factory.rs` compares both fields against those defaults.

use std::ffi::CStr;
use std::sync::atomic::{AtomicUsize, Ordering};

use eds_sys::{
    ECollectionBackendFactory, ECollectionBackendFactoryClass,
    e_collection_backend_factory_get_type,
};
use glib_sys::GType;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};

use crate::backend::JmapCollectionBackend;
use crate::prepare_mail::prepare_mail_trampoline;

/// The `BackendName` that makes the registry select this factory. Thus it is
/// the name in `[Collection]` of each JMAP `.source` file, written by hand or
/// by the setup UI.
///
/// The spelling is intentionally the same as `FACTORY_NAME` of
/// `jmap-backend-book` and `jmap-backend-cal` and the Camel protocol name.
/// They are four different namespaces, and one account uses all four.
pub const FACTORY_NAME: &CStr = c"jmap";

/// The factory from which the registry gets a [`JmapCollectionBackend`].
#[repr(C)]
pub struct JmapCollectionFactory {
    /// For GObject. The base class contains all the data of this type.
    parent: ECollectionBackendFactory,
}

/// The class struct. Unlike the instance, it is important: the two fields that
/// a collection factory subclass sets are in its parent part.
#[repr(C)]
pub struct JmapCollectionFactoryClass {
    pub parent_class: ECollectionBackendFactoryClass,
}

/// The backend `GType` from the last [`register_dynamic`] of the backend.
///
/// `GType` is `gsize`, so an `AtomicUsize` holds it exactly.
///
/// [`register_dynamic`]: jmap_backend_core::subclass::register_dynamic
static BACKEND_TYPE: AtomicUsize = AtomicUsize::new(0);

/// Records the backend type this factory should build.
///
/// The module entry point registers the backend before the factory, and calls
/// this function with the result. `G_DEFINE_DYNAMIC_TYPE` gives the same result
/// to a C backend. `class_init` runs after `e_module_load`, so the type is
/// available when the factory needs it. If `class_init` registered the backend,
/// the registration would be static. A static type stays after the module that
/// contains the code of its vfuncs is unloaded.
pub fn remember_backend_type(gtype: GType) {
    BACKEND_TYPE.store(gtype, Ordering::Release);
}

/// The type that [`class_init`](ObjectSubclass::class_init) sets as the type to
/// build.
///
/// The fallback is for a test, which uses the factory class without a module
/// load. The registry cannot cause this. A static registration is correct
/// there, because there is no module. It also prevents the alternative. For the
/// address book factory the alternative is a zero `GType`. Here it is the
/// inherited `E_TYPE_COLLECTION_BACKEND`, which is worse: `g_object_new`
/// succeeds and the account shows, but the collection never fans out.
fn backend_type() -> GType {
    match BACKEND_TYPE.load(Ordering::Acquire) {
        0 => register_static::<JmapCollectionBackend>(),
        gtype => gtype,
    }
}

// SAFETY: both structs are #[repr(C)]. They start with the instance struct and
// the class struct of ECollectionBackendFactory. ECollectionBackendFactory
// derives from GObject (through EBackendFactory and EExtension).
unsafe impl ObjectSubclass for JmapCollectionFactory {
    const NAME: &'static CStr = c"ECollectionBackendJmapFactory";
    type Instance = JmapCollectionFactory;
    type Class = JmapCollectionFactoryClass;

    fn parent_type() -> GType {
        // SAFETY: there are no arguments, and the EDS type system initialises
        // itself.
        unsafe { e_collection_backend_factory_get_type() }
    }

    fn class_init_types() -> Vec<GType> {
        // Register the backend type here, if necessary, and not in
        // `class_init`. `class_init` runs under the class-initialisation lock of
        // GLib, and the registration lock there would invert the lock order.
        // Under EDS, the module already registered the type, and this only
        // reads the atomic.
        vec![backend_type()]
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points to a new class struct of this type, which
        // starts with the struct of the parent. Both fields are in that part.
        let factory = unsafe { &mut (*class).parent_class };
        factory.factory_name = FACTORY_NAME.as_ptr();
        factory.backend_type = backend_type();

        // The third field is not about this backend. The mail account,
        // identity and transport sources are not children of this collection
        // (see `crate::prepare_mail` for the reason). `prepare_mail` is the only
        // control that a collection factory has over them. The inherited
        // default only connects the three, and gives no name to the two
        // services. This implementation chains up, then names the Camel
        // provider for them.
        factory.prepare_mail = Some(prepare_mail_trampoline);
    }
}

#[cfg(test)]
mod tests {
    use super::{backend_type, remember_backend_type};

    /// `register_static::<JmapCollectionBackend>()` cannot give this value. Thus
    /// a pass shows the recorded value, not a fallback with the same number.
    #[test]
    fn remember_backend_type_is_what_backend_type_then_reports() {
        remember_backend_type(0xdead_beef);
        assert_eq!(backend_type(), 0xdead_beef);
    }
}
