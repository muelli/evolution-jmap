// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `EBookBackendFactory` subclass — how EDS gets from a `.source` file to
//! a [`JmapBookBackend`].
//!
//! `evolution-addressbook-factory` never instantiates a backend itself. It
//! collects the children of `EBookBackendFactory` that the modules in its
//! backend directory registered, keys them by the name each one declares, and
//! hands an `ESource` to whichever answers to the `BackendName` in that
//! source's `[Address Book]` group. The base class does all of it; a subclass
//! only has to say what it is called and what to build.
//!
//! So this file is two assignments in a `class_init`, and the only thing that
//! needs any thought is where the second one gets its `GType` from — see
//! [`remember_backend_type`].

use std::ffi::CStr;
use std::sync::atomic::{AtomicUsize, Ordering};

use eds_sys::{EBookBackendFactory, EBookBackendFactoryClass, e_book_backend_factory_get_type};
use glib_sys::GType;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};

use crate::backend::JmapBookBackend;

/// What an address book's `BackendName` has to say for this factory to be the
/// one EDS picks. It is also the name the collection backend (M6) will put
/// into the sources it writes, and the directory name under
/// `~/.config/evolution/sources` has nothing to do with it.
pub const FACTORY_NAME: &CStr = c"jmap";

/// The factory EDS asks for a [`JmapBookBackend`].
#[repr(C)]
pub struct JmapBookFactory {
    /// GObject's; the base class keeps everything this type has.
    parent: EBookBackendFactory,
}

/// The class struct, which unlike the instance is not empty of interest: the
/// two fields a factory subclass exists to fill in live in the parent half.
#[repr(C)]
pub struct JmapBookFactoryClass {
    pub parent_class: EBookBackendFactoryClass,
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
/// after `e_module_load`, so by the time the factory needs the type it is
/// there. Going the other way — having `class_init` register the backend
/// itself — would register it *statically*, and a statically registered type
/// outlives the module whose code its vfuncs point into.
pub fn remember_backend_type(gtype: GType) {
    BACKEND_TYPE.store(gtype, Ordering::Release);
}

/// What [`class_init`](ObjectSubclass::class_init) installs as the type to
/// build.
///
/// The fallback is for the case that cannot happen under EDS and does happen
/// in a test: the factory class referenced without the module ever having been
/// loaded. Registering the backend statically there is right — there is no
/// module for it to belong to — and it keeps the alternative, a factory with a
/// zero `backend_type`, out of the picture. That one is a `g_object_new(0)`
/// per address book: a GLib critical, a NULL backend, and no hint as to why.
fn backend_type() -> GType {
    match BACKEND_TYPE.load(Ordering::Acquire) {
        0 => register_static::<JmapBookBackend>(),
        gtype => gtype,
    }
}

// SAFETY: both structs are #[repr(C)] and lead with the EBookBackendFactory
// instance and class structs respectively, and EBookBackendFactory derives
// from GObject (via EBackendFactory and EExtension).
unsafe impl ObjectSubclass for JmapBookFactory {
    const NAME: &'static CStr = c"EBookBackendJmapFactory";
    type Instance = JmapBookFactory;
    type Class = JmapBookFactoryClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the EDS type system initialises itself.
        unsafe { e_book_backend_factory_get_type() }
    }

    fn class_init_types() -> Vec<GType> {
        // The backend type below, in the case where `class_init` would
        // otherwise register it: that runs under GLib's class-initialisation
        // lock, which cannot take the registration lock without inverting the
        // two. Under EDS the module has already registered it and this only
        // reads the atomic.
        vec![backend_type()]
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points at a freshly allocated class struct of ours,
        // which leads with the parent's; both fields are in that half.
        let factory = unsafe { &mut (*class).parent_class };
        factory.factory_name = FACTORY_NAME.as_ptr();
        factory.backend_type = backend_type();

        // `share_subprocess` is deliberately left alone. Setting it would put
        // every JMAP address book in the session into one
        // `evolution-addressbook-factory-subprocess`, and those books belong
        // to different accounts holding different credentials; the default
        // gives each source its own process, which is a process more and a
        // blast radius less.
    }
}
