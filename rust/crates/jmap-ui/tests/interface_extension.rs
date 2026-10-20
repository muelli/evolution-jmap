// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The types that `EExtensionClass::extensible_type` can name.
//!
//! Each `EExtension` in this workspace names a *class* there. Thus no code
//! needed this answer before. The `evolution-mdn.c` of Evolution names an
//! *interface*, `E_TYPE_MAIL_READER`. With it, one extension reaches the
//! reader of the mail window and the detached message window. The snooze
//! feature of this crate now needs two extensions, one for each concrete
//! class, to reach the same two windows.
//!
//! That design depends on one ABI question: does the type system accept an
//! interface? Both gates are `g_type_is_a (G_OBJECT_TYPE (extensible),
//! class->extensible_type)`:
//!
//! - the candidate filter in `e_extensible_reload_extensions`,
//! - the check in the setter of the `extensible` property.
//!
//! `g_type_is_a` answers for interfaces and for ancestors, so the source code
//! shows that it works. But the source code is not the running type system.
//! An error is silent: an extension that does not load leaves no trace.
//!
//! This test confirms the behaviour on probe types, not on Evolution types.
//! Thus the test needs only libedataserver and no display. The probe types
//! are an interface, one object that implements it, one object that does not,
//! and an extension typed on the interface.

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

/// The last extensible for which this extension was constructed, and the
/// number of constructions since `load_extensions_for` reset the pair.
static EXTENDED: AtomicUsize = AtomicUsize::new(0);
static LOADS: AtomicUsize = AtomicUsize::new(0);

/// The probe interface, in place of `EMailReader`. It has no vfuncs, because
/// no code here dispatches through it. It must only be an interface, with
/// `GObject` as its prerequisite, as `EMailReader` has `GtkWidget`.
fn probe_interface() -> GType {
    static TYPE: OnceLock<usize> = OnceLock::new();
    *TYPE.get_or_init(|| {
        // SAFETY: the shape that `G_DEFINE_INTERFACE` registers: a child of
        // G_TYPE_INTERFACE whose class struct is only a GTypeInterface, with
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

/// A plain `GObject` that is extensible, and can implement the probe
/// interface. The two objects of the test differ only in that interface.
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

        // SAFETY: both structs are #[repr(C)] and start with the GObject structs.
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
        // SAFETY: no arguments. The type initialises itself.
        InterfaceDecl::defaults(unsafe { eds_sys::e_extensible_get_type() }),
    ]
);

probe_object!(
    ProbePlainObject,
    ProbePlainObjectClass,
    c"JmapProbePlainObject",
    // SAFETY: no arguments. The type initialises itself.
    vec![InterfaceDecl::defaults(unsafe {
        eds_sys::e_extensible_get_type()
    })]
);

/// The extension that this test examines. Its type is the interface, not a class.
#[repr(C)]
struct ProbeExtension {
    parent: EExtension,
}

#[repr(C)]
struct ProbeExtensionClass {
    parent_class: EExtensionClass,
}

// SAFETY: both structs are #[repr(C)] and start with the EExtension structs.
// EExtension derives from GObject.
unsafe impl ObjectSubclass for ProbeExtension {
    const NAME: &'static CStr = c"JmapProbeReaderExtension";
    type Instance = ProbeExtension;
    type Class = ProbeExtensionClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments. The type initialises itself.
        unsafe { e_extension_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` starts with `EExtensionClass`.
        unsafe { (*class).parent_class.extensible_type = probe_interface() };
        let object_class = class.cast::<GObjectClass>();
        // SAFETY: through its parents, `class` starts with GObjectClass.
        unsafe { (*object_class).constructed = Some(constructed) };
    }
}

/// Records that this extension was instantiated, and for which object.
unsafe extern "C" fn constructed(object: *mut GObject) {
    // SAFETY: the parent class of a live instance is initialised and alive.
    let parent = unsafe { subclass::parent_class::<GObjectClass>(ProbeExtension::parent_type()) };
    if let Some(chained) = parent.and_then(|class| class.constructed) {
        // SAFETY: GObject passes a live instance being constructed.
        unsafe { chained(object) };
    }
    // SAFETY: a live extension. The property setter accepted its extensible
    // before `constructed` runs.
    let extensible = unsafe { e_extension_get_extensible(object.cast::<EExtension>()) };
    EXTENDED.store(extensible as usize, Ordering::SeqCst);
    LOADS.fetch_add(1, Ordering::SeqCst);
}

/// Instantiate `gtype`, load its extensions, and return what the probe
/// extension recorded: the number of constructions, and for which object.
fn load_extensions_for(gtype: GType) -> (usize, usize, usize) {
    LOADS.store(0, Ordering::SeqCst);
    EXTENDED.store(0, Ordering::SeqCst);
    // SAFETY: a registered GObject type with no construct properties, so the
    // count is zero and both arrays are NULL. The object is extensible, the
    // only precondition of `e_extensible_load_extensions`.
    let (loads, extended, object) = unsafe {
        let object = g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null());
        e_extensible_load_extensions(object.cast());
        let seen = (
            LOADS.load(Ordering::SeqCst),
            EXTENDED.load(Ordering::SeqCst),
        );
        (seen.0, seen.1, object as usize)
    };
    // SAFETY: the reference that `g_object_new_with_properties` returned,
    // dropped after the test recorded the extensions that it owns.
    unsafe { g_object_unref(object as *mut GObject) };
    (loads, extended, object)
}

#[test]
fn an_extension_typed_on_an_interface_loads_for_an_implementer_only() {
    // The registration puts the extension in the walk of `e_type_traverse`
    // through the descendants of E_TYPE_EXTENSION. Only thus can the objects
    // below find it.
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

    // The negative half: without the interface, the extension must not load.
    // Otherwise the filter accepts all extensibles, and the positive half
    // above proves nothing.
    let other = subclass::register_static::<ProbePlainObject>();
    let (loads, _, _) = load_extensions_for(other);
    assert_eq!(
        loads, 0,
        "the extension loaded for a type that does not implement its interface"
    );
}
