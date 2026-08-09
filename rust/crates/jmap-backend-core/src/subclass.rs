// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Registering a Rust-declared type with the GObject type system.
//!
//! This is the hand-written equivalent of `G_DEFINE_TYPE`: describe the
//! instance and class structs, point at a parent `GType`, and GObject
//! allocates instances of that size with the parent's class already
//! initialised in the leading bytes. The `glib` crate's `subclass` module
//! does this too, but only for types whose parent it wraps — EDS types are
//! not among them, and eds-sys deliberately stops at the raw ABI.
//!
//! Both registration flavours exist because backends need both: an EDS module
//! registers its types against the `GTypeModule` it is loaded as, so they can
//! be unloaded again, while anything created outside a module (tests, and the
//! Camel provider's own types) registers statically. A type may also declare
//! the interfaces it implements — the `G_IMPLEMENT_INTERFACE` half of
//! `G_DEFINE_TYPE_WITH_CODE` — which registration adds before it hands the
//! `GType` back, because an interface added later is one the class never saw.

use std::ffi::CStr;
use std::mem::size_of;
use std::ptr;
use std::sync::Mutex;

use glib_sys::{GType, gpointer};
use gobject_sys::{
    GInterfaceInfo, GInterfaceInitFunc, GObject, GObjectClass, GTypeInfo, GTypeInstance,
    GTypeModule, g_type_add_interface_static, g_type_class_peek, g_type_from_name,
    g_type_module_add_interface, g_type_module_register_type, g_type_name, g_type_register_static,
};

use crate::trampoline::{guard, log_critical};

/// A type this crate can register.
///
/// # Safety
///
/// GObject reads and writes through these structs using the layout it derives
/// from `size_of`, so implementations must guarantee:
///
/// - `Instance` is `#[repr(C)]` and its first field is the parent type's
///   instance struct.
/// - `Class` is `#[repr(C)]` and its first field is the parent type's class
///   struct.
/// - [`parent_type`](ObjectSubclass::parent_type) returns the `GType` those
///   two parent structs actually belong to, and that type derives from
///   `GObject` — registration overrides `GObjectClass.finalize` through the
///   leading bytes of `Class`.
///
/// Getting any of that wrong misplaces every field and vfunc slot, which the
/// compiler cannot catch — it is the same hazard `eds-sys`'s `g_type_query`
/// layout test guards against on the C side.
pub unsafe trait ObjectSubclass: Sized {
    /// Must be unique process-wide; GObject aborts on a duplicate.
    const NAME: &'static CStr;

    type Instance;
    type Class;

    fn parent_type() -> GType;

    /// The interfaces this type implements, added to it before registration
    /// hands the `GType` back.
    ///
    /// Timing is the reason this is declared rather than done by the caller.
    /// An interface has to be on the type before its class is initialised:
    /// `g_object_class_override_property`, which is how an implementer
    /// satisfies an interface's properties, runs in
    /// [`class_init`](ObjectSubclass::class_init) and only finds properties of
    /// interfaces the type already implements. A caller adding the interface
    /// after registration returned would be holding a `GType` that, for one
    /// window, implements nothing — and anything that referenced its class in
    /// that window would fix the omission permanently.
    ///
    /// Each entry says how the interface's vtable is filled:
    /// [`InterfaceDecl::defaults`] for one this type only claims, and
    /// [`InterfaceDecl::filled_by`] for one whose vfunc slots it fills.
    fn interfaces() -> Vec<InterfaceDecl> {
        Vec::new()
    }

    /// Runs once, before any instance exists. This is where vfunc slots in
    /// `Class` (and in the parent class reachable through it) get overridden.
    ///
    /// # Safety
    ///
    /// `class` points at a freshly allocated class struct of this type, with
    /// the parent class already initialised.
    unsafe fn class_init(_class: *mut Self::Class) {}

    /// Runs for each instance, after the parent's `instance_init`.
    ///
    /// # Safety
    ///
    /// `instance` points at a zeroed instance struct of this type.
    unsafe fn instance_init(_instance: *mut Self::Instance) {}

    /// Runs once per instance, when the last reference to it is dropped and
    /// before the parent's `finalize`. This is where anything
    /// [`instance_init`](ObjectSubclass::instance_init) created is destroyed —
    /// GObject frees the instance struct itself without running a Rust
    /// destructor over it, so a [`Slot`](crate::instance::Slot) that is not
    /// cleared here leaks its contents.
    ///
    /// Chaining up is not this method's job: registration does it, whatever
    /// happens here.
    ///
    /// # Safety
    ///
    /// `instance` points at an instance struct of this type that is being
    /// finalized; nothing else can still reach it.
    unsafe fn finalize(_instance: *mut Self::Instance) {}
}

/// One interface a type declares, and how the copy of that interface's vtable
/// GObject gives the type is filled in.
///
/// Built by one of the two constructors rather than by naming its fields: the
/// choice between them is the whole content of the type, and a caller that
/// could write `init: None` beside an interface with vfuncs would be declaring
/// an implementation that dispatches to whatever the interface's `default_init`
/// left there — for Camel's interfaces, a NULL the wrapper function calls
/// straight through.
pub struct InterfaceDecl {
    gtype: GType,
    init: GInterfaceInitFunc,
}

impl InterfaceDecl {
    /// An interface this type claims without filling any slot of its own: the
    /// implementation is whatever the interface's `default_init` put in the
    /// vtable.
    ///
    /// Right for an interface that is all properties, which the implementer
    /// satisfies in [`class_init`](ObjectSubclass::class_init) with
    /// `g_object_class_override_property` and not through the vtable at all —
    /// `CamelNetworkSettings`, the one this hook was first written for, declares
    /// no vfuncs.
    pub fn defaults(gtype: GType) -> Self {
        Self { gtype, init: None }
    }

    /// An interface whose vtable `I` fills in, once, when GObject initialises
    /// this type's class.
    pub fn filled_by<I: InterfaceImpl>() -> Self {
        Self {
            gtype: I::gtype(),
            init: Some(interface_init_trampoline::<I>),
        }
    }
}

/// Filling in one implementing type's copy of one interface's vtable — the
/// `G_IMPLEMENT_INTERFACE` init function, in Rust.
///
/// This is a trait of its own, implemented by a type *beside* the class rather
/// than by the class itself, because a class may implement several interfaces
/// and would then need several of these. The Camel provider's store is already
/// that shape: `CamelSubscribable` is one interface it fills, and it is not
/// going to be the last.
///
/// # Why not from `class_init`
///
/// Not because it cannot be done: `g_type_interface_peek` inside
/// [`class_init`](ObjectSubclass::class_init) does hand back this type's own
/// copy of the vtable, and a slot written through it does survive — checked,
/// rather than assumed, before this hook was written.
///
/// It works by an ordering GLib does not promise. `gtype.c` happens to
/// base-initialise a type's interface vtables before it calls the class's
/// initialiser and to run the `interface_init` functions after it; nothing in
/// the documentation says so, and `g_type_interface_peek` is specified in terms
/// of a type's interfaces, not of a class that is halfway through being built.
/// `GInterfaceInfo.interface_init` is the documented contract for exactly this,
/// so that is what we take — and taking it also means the implementer is handed
/// a typed `*mut Vtable` under a panic guard instead of casting a `gpointer` at
/// the call site, with the filling declared next to the interface rather than
/// among the class's own overrides.
///
/// # Safety
///
/// GObject writes through `Vtable` using the layout the C interface struct has,
/// so implementations must guarantee:
///
/// - `Vtable` is `#[repr(C)]`, is the binding of the interface struct
///   [`gtype`](InterfaceImpl::gtype) names, and leads with `GTypeInterface`.
/// - [`gtype`](InterfaceImpl::gtype) really is that interface's `GType`.
///
/// A mismatch puts a function pointer at some other slot's offset, which the
/// compiler cannot catch and which surfaces as the wrong vfunc being called
/// with the wrong arguments.
pub unsafe trait InterfaceImpl {
    /// The interface's vtable struct — `CamelSubscribableInterface` and the
    /// like.
    type Vtable;

    /// The interface this fills. A type accessor rather than a constant,
    /// because a `GType` is only a number once the interface has registered
    /// itself.
    fn gtype() -> GType;

    /// Writes this type's implementations into the slots it fills, leaving the
    /// rest at the interface's own defaults.
    ///
    /// # Safety
    ///
    /// `vtable` points at the implementing type's own copy of the interface's
    /// vtable, freshly initialised from the interface's defaults and reachable
    /// by nothing else yet.
    unsafe fn interface_init(vtable: *mut Self::Vtable);
}

/// GObject calls this from inside `g_type_class_ref`, while it holds the type
/// system's global lock, so it is guarded exactly like
/// [`class_init_trampoline`]: a panic unwinding from here would abort the
/// process rather than break one account.
///
/// A caught panic leaves the vtable however far the init got — which for a slot
/// never reached is the interface's own default. That is the honest outcome:
/// the alternative, putting the defaults back, would mean copying a vtable this
/// code does not know the size of.
unsafe extern "C" fn interface_init_trampoline<I: InterfaceImpl>(
    vtable: gpointer,
    _data: gpointer,
) {
    // SAFETY: the interface is registered — `filled_by` asked it for its GType
    // — so `g_type_name` returns its name rather than NULL.
    let name = unsafe { CStr::from_ptr(g_type_name(I::gtype())) };
    let context = format!("{}::interface_init", name.to_string_lossy());

    guard(&context, (), || unsafe {
        I::interface_init(vtable.cast::<I::Vtable>())
    });
}

/// Registers `T` as a static type, or returns the existing `GType` if it has
/// already been registered.
///
/// Idempotence is not a convenience: a second `g_type_register_static` under
/// the same name is a fatal GLib error, and a process does reach the same
/// registration twice — a test suite from two threads, a Camel provider from
/// two `camel_provider_module_init`s.
pub fn register_static<T: ObjectSubclass>() -> GType {
    // SAFETY: a null GTypeModule selects the static path, which has no
    // pointer requirements of its own.
    unsafe { register::<T>(ptr::null_mut()) }
}

/// Registers `T` against the `GTypeModule` EDS loaded us as, so the type is
/// unregistered if the module is unloaded.
///
/// Call this on *every* load of the module, not only the first: unusing a
/// module marks the types it registered as unloaded, and GLib will not hand
/// one back until a later load has registered it again.
///
/// # Safety
///
/// `module` must be a valid `GTypeModule`, i.e. the pointer EDS passed to
/// `e_module_load`.
pub unsafe fn register_dynamic<T: ObjectSubclass>(module: *mut GTypeModule) -> GType {
    debug_assert!(!module.is_null(), "register_dynamic needs a GTypeModule");
    unsafe { register::<T>(module) }
}

/// Serialises the check-then-register pair. GObject's own registration is
/// thread-safe, but "is it there yet?" followed by "register it" is not, and
/// losing that race aborts the process.
static REGISTRATION: Mutex<()> = Mutex::new(());

unsafe fn register<T: ObjectSubclass>(module: *mut GTypeModule) -> GType {
    // Resolved before the lock is taken, not inside the GTypeInfo below: a
    // parent_type() that registers another of our types — which is how a
    // hierarchy declared entirely in Rust bootstraps itself — would otherwise
    // deadlock on this very much non-reentrant mutex. The same goes for the
    // interfaces, which are resolved through type accessors of their own.
    let parent = T::parent_type();
    let interfaces = T::interfaces();

    // A poisoned lock means some other registration panicked; the type system
    // itself is untouched by that, so carry on rather than panic in turn.
    let _guard = REGISTRATION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // Only on the static path. A second `g_type_register_static` under the
    // same name is a fatal GLib error, so an already-registered type has to be
    // handed straight back; `g_type_module_register_type` is the opposite. It
    // *has* to be called again on every load of the module, because unusing a
    // module marks every type it registered as unloaded, and GLib refuses the
    // module — and aborts the process, "Could not reload previously loaded
    // plugin", as soon as one of those types is asked for — until a second
    // registration has marked them loaded again.
    if module.is_null() {
        // SAFETY: NAME is a 'static NUL-terminated string.
        let existing = unsafe { g_type_from_name(T::NAME.as_ptr()) };
        if existing != 0 {
            return existing;
        }
    }

    let class_size = u16::try_from(size_of::<T::Class>())
        .unwrap_or_else(|_| panic!("{:?}: class struct exceeds GObject's 64 KiB", T::NAME));
    let instance_size = u16::try_from(size_of::<T::Instance>())
        .unwrap_or_else(|_| panic!("{:?}: instance struct exceeds GObject's 64 KiB", T::NAME));

    let info = GTypeInfo {
        class_size,
        base_init: None,
        base_finalize: None,
        class_init: Some(class_init_trampoline::<T>),
        class_finalize: None,
        class_data: ptr::null(),
        instance_size,
        n_preallocs: 0,
        instance_init: Some(instance_init_trampoline::<T>),
        value_table: ptr::null(),
    };

    // SAFETY: `info` describes structs whose layout the ObjectSubclass
    // contract pins to the parent's, and it only has to outlive the call.
    let gtype = unsafe {
        if module.is_null() {
            g_type_register_static(parent, T::NAME.as_ptr(), &info, 0)
        } else {
            g_type_module_register_type(module, parent, T::NAME.as_ptr(), &info, 0)
        }
    };

    // Before the type is handed back, and so before anything can reference its
    // class: an interface added after class_init has run is an interface whose
    // properties the class never overrode.
    for interface in interfaces {
        let interface_info = GInterfaceInfo {
            interface_init: interface.init,
            interface_finalize: None,
            interface_data: ptr::null_mut(),
        };

        // SAFETY: `gtype` was just registered and `interface.gtype` came from a
        // type accessor; `interface_info` outlives both calls. On the static
        // path the type is new, so the interface cannot already be on it; on
        // the dynamic path `g_type_module_add_interface` is documented to be
        // called again on every load, like the registration above.
        unsafe {
            if module.is_null() {
                g_type_add_interface_static(gtype, interface.gtype, &interface_info);
            } else {
                g_type_module_add_interface(module, gtype, interface.gtype, &interface_info);
            }
        }
    }

    gtype
}

/// GObject calls these from C, so they are guarded: a panic in a user
/// `class_init` would otherwise unwind through the type system's own frames
/// while it holds its global lock.
unsafe extern "C" fn class_init_trampoline<T: ObjectSubclass>(class: gpointer, _data: gpointer) {
    // Installed before `T::class_init` runs, so a subclass that wants the slot
    // for itself can still take it — and installed unconditionally, because a
    // type with nothing to destroy pays only an empty call.
    //
    // SAFETY: `class` leads with the parent's class struct, which the trait's
    // contract requires to derive from GObjectClass.
    unsafe { (*class.cast::<GObjectClass>()).finalize = Some(finalize_trampoline::<T>) };

    guard("class_init", (), || unsafe {
        T::class_init(class.cast::<T::Class>())
    });
}

/// Drops what `instance_init` created, then hands the instance to the parent,
/// which is what eventually frees it.
///
/// The chain-up is outside the guard on purpose: a panic in `T::finalize` is
/// already a bug, and skipping the parent's finalize would turn it into a leak
/// of every instance from then on — including, for an `EBookMetaBackend`, the
/// `ESource` and the offline cache.
unsafe extern "C" fn finalize_trampoline<T: ObjectSubclass>(object: *mut GObject) {
    guard("finalize", (), || unsafe {
        T::finalize(object.cast::<T::Instance>())
    });

    // The parent class rather than `g_type_class_peek_parent` of the
    // instance's class: a further subclass of ours would make that one point
    // back at this same trampoline and recurse until the stack ran out.
    //
    // SAFETY: an instance of this type exists, so its class does, so the class
    // it derives from is initialised and alive.
    let parent = unsafe { g_type_class_peek(T::parent_type()) }.cast::<GObjectClass>();
    match unsafe { parent.as_ref() }.and_then(|class| class.finalize) {
        // SAFETY: this is the parent's own finalize, called on an instance of
        // a type derived from it, which is what chaining up means in C.
        Some(finalize) => unsafe { finalize(object) },
        None => log_critical(&format!(
            "{:?}: the parent class has no finalize to chain up to; the instance is leaked",
            T::NAME
        )),
    }
}

unsafe extern "C" fn instance_init_trampoline<T: ObjectSubclass>(
    instance: *mut GTypeInstance,
    _class: gpointer,
) {
    guard("instance_init", (), || unsafe {
        T::instance_init(instance.cast::<T::Instance>())
    });
}
