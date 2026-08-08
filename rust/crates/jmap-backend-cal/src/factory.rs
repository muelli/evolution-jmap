// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `ECalBackendFactory` subclass — how EDS gets from a `.source` file to a
//! [`JmapCalBackend`].
//!
//! `evolution-calendar-factory` never instantiates a backend itself. It collects
//! the children of `ECalBackendFactory` that the modules in its backend
//! directory registered, keys them by what each one declares, and hands an
//! `ESource` to whichever answers for the `BackendName` in that source's
//! `[Calendar]` group. The base class does all of it; a subclass only fills in
//! three class fields.
//!
//! Three, not the address book's two — and the extra one is why this file is
//! not `jmap-backend-book`'s with the names changed. A calendar factory also
//! declares the *kind* of component it serves; EDS keys the factory by name and
//! kind together, and passes the kind on to the backend as its `kind` construct
//! property. See [`COMPONENT_KIND`].

use std::ffi::CStr;
use std::sync::atomic::{AtomicUsize, Ordering};

use eds_sys::{
    ECalBackendFactory, ECalBackendFactoryClass, I_CAL_VEVENT_COMPONENT, ICalComponentKind,
    e_cal_backend_factory_get_type,
};
use glib_sys::GType;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};

use crate::backend::JmapCalBackend;

/// What a calendar's `BackendName` has to say for this factory to be the one EDS
/// picks. It is also the name the collection backend (M6) will put into the
/// sources it writes.
///
/// The same string as the address book's factory name, deliberately: the two
/// factories are collected by different processes into different hash tables, so
/// one account can name one backend for both of its collections.
pub const FACTORY_NAME: &CStr = c"jmap";

/// The one component kind this factory serves.
///
/// `ECalBackendFactory` keys itself by name *and* kind — the hash key EDS looks
/// a source up by is `"jmap:VEVENT"` — so declaring only events is what makes a
/// task list or a memo list asking for `BackendName=jmap` find no factory at
/// all. That is the honest answer: `jmap-cal-sync` maps `CalendarEvent`s, and
/// JMAP has no standardised task or note type to map the other two onto.
/// Registering factories for them would produce backends that connect, sync
/// nothing, and look broken rather than absent.
///
/// It reaches the backend, too: the base class passes it to `g_object_new` as
/// `kind`, which is what `e_cal_backend_get_kind` reports to every client.
pub const COMPONENT_KIND: ICalComponentKind = I_CAL_VEVENT_COMPONENT;

/// The factory EDS asks for a [`JmapCalBackend`].
#[repr(C)]
pub struct JmapCalFactory {
    /// GObject's; the base class keeps everything this type has.
    parent: ECalBackendFactory,
}

/// The class struct, which unlike the instance is not empty of interest: the
/// three fields a factory subclass exists to fill in live in the parent half.
#[repr(C)]
pub struct JmapCalFactoryClass {
    pub parent_class: ECalBackendFactoryClass,
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
/// there. Going the other way — having `class_init` register the backend itself
/// — would register it *statically*, and a statically registered type outlives
/// the module whose code its vfuncs point into.
pub fn remember_backend_type(gtype: GType) {
    BACKEND_TYPE.store(gtype, Ordering::Release);
}

/// What [`class_init`](ObjectSubclass::class_init) installs as the type to
/// build.
///
/// The fallback is for the case that cannot happen under EDS and does happen in
/// a test: the factory class referenced without the module ever having been
/// loaded. Registering the backend statically there is right — there is no
/// module for it to belong to — and it keeps the alternative, a factory with a
/// zero `backend_type`, out of the picture. That one is a `g_object_new(0)` per
/// calendar: a GLib critical, a NULL backend, and no hint as to why.
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

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points at a freshly allocated class struct of ours,
        // which leads with the parent's; all three fields are in that half.
        let factory = unsafe { &mut (*class).parent_class };
        factory.factory_name = FACTORY_NAME.as_ptr();
        factory.component_kind = COMPONENT_KIND;
        factory.backend_type = backend_type();

        // `share_subprocess` is deliberately left alone, as in the address
        // book's factory. Setting it would put every JMAP calendar in the
        // session into one `evolution-calendar-factory-subprocess`, and those
        // calendars belong to different accounts holding different credentials;
        // the default gives each source its own process, which is a process
        // more and a blast radius less.
    }
}
