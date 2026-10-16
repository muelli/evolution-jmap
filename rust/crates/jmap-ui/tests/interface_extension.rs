// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What `EExtensionClass::extensible_type` is allowed to name.
//!
//! Every `EExtension` in this workspace names a *class* there, so nothing so
//! far has needed the answer. Evolution's own `evolution-mdn.c` names an
//! *interface*, `E_TYPE_MAIL_READER`, and reaches both the mail window's
//! reader and the detached message window with one extension that way; this
//! crate's snooze feature currently needs two extensions to reach the same
//! two windows, one per concrete class.
//!
//! The ABI question that design rests on is whether an interface is accepted
//! at all. Both gates are `g_type_is_a (G_OBJECT_TYPE (extensible),
//! class->extensible_type)` -- the candidate filter in
//! `e_extensible_reload_extensions` and the check in the `extensible`
//! property's setter -- and `g_type_is_a` answers for interfaces as well as
//! for ancestors, so by reading it works. Reading is not the same as the
//! running type system agreeing, and getting it wrong is silent: an
//! extension that never loads leaves no trace at all.
//!
//! So this pins it, on probe types rather than Evolution's, which keeps the
//! test to libedataserver and needs no display: an interface, one object
//! implementing it and one not, and an extension typed on the interface.

use std::ffi::CStr;
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use eds_sys::{
    EExtension, EExtensionClass, e_extensible_load_extensions, e_extension_get_extensible,
    e_extension_get_type,
};
use glib_sys::GType;
use gobject_sys::{
    G_TYPE_INTERFACE, G_TYPE_OBJECT, GObject, GObjectClass, GTypeInfo, GTypeInterface,
    g_object_new_with_properties, g_object_unref, g_type_interface_add_prerequisite,
    g_type_register_static,
};
use jmap_backend_core::subclass::{self, InterfaceDecl, ObjectSubclass};

/// The last extensible this extension was constructed for, and how many
/// times it was constructed since `load_extensions_for` reset the pair.
static EXTENDED: AtomicUsize = AtomicUsize::new(0);
static LOADS: AtomicUsize = AtomicUsize::new(0);

/// The probe interface, standing in for `EMailReader`: no vfuncs, because
/// nothing here dispatches through it. What matters is only that it is an
/// interface, with `GObject` as its prerequisite the way `EMailReader` has
/// `GtkWidget`.
fn probe_interface() -> GType {
    static TYPE: OnceLock<usize> = OnceLock::new();
    *TYPE.get_or_init(|| {
        // SAFETY: the shape `G_DEFINE_INTERFACE` registers: a child of
        // G_TYPE_INTERFACE whose class struct is a bare GTypeInterface, with
        // no initialisers and no instances of its own.
        let gtype = unsafe {
            let info = GTypeInfo {
                class_size: size_of::<GTypeInterface>() as u16,
                ..std::mem::zeroed()
            };
            let gtype =
                g_type_register_static(G_TYPE_INTERFACE, c"JmapProbeReader".as_ptr(), &info, 0);
            g_type_interface_add_prerequisite(gtype, G_TYPE_OBJECT);
            gtype
        };
        assert_ne!(gtype, 0, "the probe interface was not registered");
        gtype as usize
    }) as GType
}

/// A plain `GObject` that is extensible, and optionally implements the probe
/// interface: the two halves of the question, differing in nothing else.
macro_rules! probe_object {
    ($rust:ident, $class:ident, $name:literal, $interfaces:expr) => {
        #[repr(C)]
        struct $rust {
            parent: GObject,
        }

        #[repr(C)]
        struct $class {
            parent_class: GObjectClass,
        }

        // SAFETY: both structs are #[repr(C)] and lead with GObject's own.
        unsafe impl ObjectSubclass for $rust {
            const NAME: &'static CStr = $name;
            type Instance = $rust;
            type Class = $class;

            fn parent_type() -> GType {
                G_TYPE_OBJECT
            }

            fn interfaces() -> Vec<InterfaceDecl> {
                $interfaces
            }
        }
    };
}

probe_object!(
    ProbeReaderObject,
    ProbeReaderObjectClass,
    c"JmapProbeReaderObject",
    vec![
        InterfaceDecl::defaults(probe_interface()),
        // SAFETY: no arguments, and the type initialises itself.
        InterfaceDecl::defaults(unsafe { eds_sys::e_extensible_get_type() }),
    ]
);

probe_object!(
    ProbePlainObject,
    ProbePlainObjectClass,
    c"JmapProbePlainObject",
    // SAFETY: no arguments, and the type initialises itself.
    vec![InterfaceDecl::defaults(unsafe {
        eds_sys::e_extensible_get_type()
    })]
);

/// The extension under test: typed on the interface, not on either class.
#[repr(C)]
struct ProbeExtension {
    parent: EExtension,
}

#[repr(C)]
struct ProbeExtensionClass {
    parent_class: EExtensionClass,
}

// SAFETY: both structs are #[repr(C)] and lead with EExtension's own structs;
// EExtension derives from GObject.
unsafe impl ObjectSubclass for ProbeExtension {
    const NAME: &'static CStr = c"JmapProbeReaderExtension";
    type Instance = ProbeExtension;
    type Class = ProbeExtensionClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_extension_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` leads with `EExtensionClass`.
        unsafe { (*class).parent_class.extensible_type = probe_interface() };
        let object_class = class.cast::<GObjectClass>();
        // SAFETY: transitively leads with GObjectClass.
        unsafe { (*object_class).constructed = Some(constructed) };
    }
}

/// Records that this extension was instantiated, and for whom.
unsafe extern "C" fn constructed(object: *mut GObject) {
    // SAFETY: the parent class of a live instance is initialised and alive.
    let parent = unsafe { subclass::parent_class::<GObjectClass>(ProbeExtension::parent_type()) };
    if let Some(chained) = parent.and_then(|class| class.constructed) {
        // SAFETY: GObject passes a live instance being constructed.
        unsafe { chained(object) };
    }
    // SAFETY: a live extension, whose extensible the property setter has
    // already accepted by the time `constructed` runs.
    let extensible = unsafe { e_extension_get_extensible(object.cast::<EExtension>()) };
    EXTENDED.store(extensible as usize, Ordering::SeqCst);
    LOADS.fetch_add(1, Ordering::SeqCst);
}

/// Instantiate `gtype`, load its extensions, and answer what the probe
/// extension saw: how many times it was constructed, and for which object.
fn load_extensions_for(gtype: GType) -> (usize, usize, usize) {
    LOADS.store(0, Ordering::SeqCst);
    EXTENDED.store(0, Ordering::SeqCst);
    // SAFETY: a registered GObject type with no construct properties, so the
    // count is zero and both arrays NULL; the object is extensible, which is
    // `e_extensible_load_extensions`'s only precondition.
    let (loads, extended, object) = unsafe {
        let object = g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null());
        e_extensible_load_extensions(object.cast());
        let seen = (
            LOADS.load(Ordering::SeqCst),
            EXTENDED.load(Ordering::SeqCst),
        );
        (seen.0, seen.1, object as usize)
    };
    // SAFETY: the reference `g_object_new_with_properties` returned, dropped
    // after the extensions it owns have been observed.
    unsafe { g_object_unref(object as *mut GObject) };
    (loads, extended, object)
}

#[test]
fn an_extension_typed_on_an_interface_loads_for_an_implementer_only() {
    // Registering the extension is what puts it in `e_type_traverse`'s walk
    // of E_TYPE_EXTENSION's descendants, which is how either object below
    // could find it at all.
    assert_ne!(
        subclass::register_static::<ProbeExtension>(),
        0,
        "the probe extension type was not registered"
    );

    let implementer = subclass::register_static::<ProbeReaderObject>();
    let (loads, extended, object) = load_extensions_for(implementer);
    assert_eq!(
        loads, 1,
        "an EExtension naming an interface did not load for a type implementing it"
    );
    assert_eq!(
        extended, object,
        "the extension was handed something other than the object it was loaded for"
    );

    // The negative half: without the interface, the same extension must not
    // be instantiated, or the filter would be accepting every extensible and
    // the positive half above would prove nothing.
    let other = subclass::register_static::<ProbePlainObject>();
    let (loads, _, _) = load_extensions_for(other);
    assert_eq!(
        loads, 0,
        "the extension loaded for a type that does not implement its interface"
    );
}
