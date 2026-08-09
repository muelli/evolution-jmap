// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `ECollectionBackendFactory` subclass — how `evolution-source-registry`
//! gets from an account's `.source` file to a [`JmapCollectionBackend`].
//!
//! The registry server is an `EDataFactory` and an `EExtensible` at once, and
//! those two roles are the whole mechanism. As an extensible it instantiates one
//! of every registered `EExtension` whose `extensible_type` is
//! `E_TYPE_SOURCE_REGISTRY_SERVER` — which every `ECollectionBackendFactory` is,
//! inherited, so nothing here registers anything *with* the server. As a data
//! factory it files those extensions by the key each one reports and hands an
//! `ESource` to whichever answers to the `BackendName` in its `[Collection]`
//! group.
//!
//! So this file is the same two assignments as `jmap-backend-book`'s factory,
//! and the difference is what happens if they are missing.
//! `e_collection_backend_factory_class_init` fills both fields in with working
//! values: `factory_name` is `"none"` and `backend_type` is
//! `E_TYPE_COLLECTION_BACKEND`. An unfinished factory is therefore not one that
//! errors — it is one filed under a name no account asks for, or one that builds
//! EDS's own do-nothing collection backend and gives the user an account that
//! connects to nothing and says nothing. `tests/factory.rs` holds both against
//! those defaults by name.

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

/// What an account's `BackendName` has to say for this factory to be the one the
/// registry picks — and so the name `[Collection]` carries in every JMAP
/// `.source` file, hand-written or written by M7's setup UI.
///
/// The same spelling as `jmap-backend-book`'s and `jmap-backend-cal`'s
/// `FACTORY_NAME` and Camel's protocol name, deliberately: they are four
/// different namespaces, and one account is all four at once.
pub const FACTORY_NAME: &CStr = c"jmap";

/// The factory the registry asks for a [`JmapCollectionBackend`].
#[repr(C)]
pub struct JmapCollectionFactory {
    /// GObject's; the base class keeps everything this type has.
    parent: ECollectionBackendFactory,
}

/// The class struct, which unlike the instance is not empty of interest: the two
/// fields a collection factory subclass exists to fill in live in the parent
/// half.
#[repr(C)]
pub struct JmapCollectionFactoryClass {
    pub parent_class: ECollectionBackendFactoryClass,
}

/// The backend `GType` the last [`register_dynamic`] of it produced.
///
/// `GType` is `gsize`, so an `AtomicUsize` holds one exactly.
///
/// [`register_dynamic`]: jmap_backend_core::subclass::register_dynamic
static BACKEND_TYPE: AtomicUsize = AtomicUsize::new(0);

/// Records the backend type this factory should build.
///
/// The module entry point registers the backend before the factory and calls
/// this with the result, which is the Rust spelling of what
/// `G_DEFINE_DYNAMIC_TYPE` gives a C backend for free: `class_init` runs long
/// after `e_module_load`, so by the time the factory needs the type it is there.
/// Going the other way — having `class_init` register the backend itself — would
/// register it *statically*, and a statically registered type outlives the module
/// whose code its vfuncs point into.
pub fn remember_backend_type(gtype: GType) {
    BACKEND_TYPE.store(gtype, Ordering::Release);
}

/// What [`class_init`](ObjectSubclass::class_init) installs as the type to build.
///
/// The fallback is for the case that cannot happen under the registry and does
/// happen in a test: the factory class referenced without the module ever having
/// been loaded. Registering the backend statically there is right — there is no
/// module for it to belong to — and it keeps the alternative out of the picture.
/// That alternative is not a zero `GType` here, as it is for the address book
/// factory, but the *inherited* `E_TYPE_COLLECTION_BACKEND`, which is worse: a
/// `g_object_new` that succeeds, an account that appears, and a collection that
/// never fans out.
fn backend_type() -> GType {
    match BACKEND_TYPE.load(Ordering::Acquire) {
        0 => register_static::<JmapCollectionBackend>(),
        gtype => gtype,
    }
}

// SAFETY: both structs are #[repr(C)] and lead with the ECollectionBackendFactory
// instance and class structs respectively, and ECollectionBackendFactory derives
// from GObject (via EBackendFactory and EExtension).
unsafe impl ObjectSubclass for JmapCollectionFactory {
    const NAME: &'static CStr = c"ECollectionBackendJmapFactory";
    type Instance = JmapCollectionFactory;
    type Class = JmapCollectionFactoryClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the EDS type system initialises itself.
        unsafe { e_collection_backend_factory_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points at a freshly allocated class struct of ours,
        // which leads with the parent's; both fields are in that half.
        let factory = unsafe { &mut (*class).parent_class };
        factory.factory_name = FACTORY_NAME.as_ptr();
        factory.backend_type = backend_type();

        // The third field, and the one that is not about this backend at all:
        // the mail account, identity and transport sources are not children of
        // this collection (see `crate::prepare_mail` for why they cannot be),
        // and `prepare_mail` is the whole of what a collection factory gets to
        // say about them. The inherited default joins the three together and
        // stops there, leaving both services nameless; ours names the Camel
        // provider that serves them, after chaining up.
        factory.prepare_mail = Some(prepare_mail_trampoline);
    }
}
