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
//! Camel provider's own types) registers statically.

use std::ffi::CStr;
use std::mem::size_of;
use std::ptr;
use std::sync::Mutex;

use glib_sys::{GType, gpointer};
use gobject_sys::{
    GObject, GObjectClass, GTypeInfo, GTypeInstance, GTypeModule, g_type_class_peek,
    g_type_from_name, g_type_module_register_type, g_type_register_static,
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

/// Registers `T` as a static type, or returns the existing `GType` if it has
/// already been registered.
///
/// Idempotence is not a convenience: a second `g_type_register_static` under
/// the same name is a fatal GLib error, and module entry points do get
/// reached more than once per process.
pub fn register_static<T: ObjectSubclass>() -> GType {
    // SAFETY: a null GTypeModule selects the static path, which has no
    // pointer requirements of its own.
    unsafe { register::<T>(ptr::null_mut()) }
}

/// Registers `T` against the `GTypeModule` EDS loaded us as, so the type is
/// unregistered if the module is unloaded.
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
    // deadlock on this very much non-reentrant mutex.
    let parent = T::parent_type();

    // A poisoned lock means some other registration panicked; the type system
    // itself is untouched by that, so carry on rather than panic in turn.
    let _guard = REGISTRATION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // SAFETY: NAME is a 'static NUL-terminated string.
    let existing = unsafe { g_type_from_name(T::NAME.as_ptr()) };
    if existing != 0 {
        return existing;
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
    unsafe {
        if module.is_null() {
            g_type_register_static(parent, T::NAME.as_ptr(), &info, 0)
        } else {
            g_type_module_register_type(module, parent, T::NAME.as_ptr(), &info, 0)
        }
    }
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
