// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Registration of a Rust-declared type with the GObject type system.
//!
//! This is the hand-written equivalent of `G_DEFINE_TYPE`. You describe the
//! instance and class structs and give a parent `GType`. GObject then
//! allocates instances of that size, with the parent's class initialised in
//! the leading bytes. The `glib` crate's `subclass` module also does this,
//! but only for types whose parent it wraps. It does not wrap EDS types, and
//! eds-sys intentionally gives only the raw ABI.
//!
//! Backends need both registration types. An EDS module registers its types
//! against the `GTypeModule` that it is loaded as, so they can be unloaded.
//! Types created outside a module (tests, and the Camel provider's own types)
//! register statically. A type can also declare the interfaces it implements
//! (the `G_IMPLEMENT_INTERFACE` part of `G_DEFINE_TYPE_WITH_CODE`).
//! Registration adds them before it returns the `GType`, because the class
//! never sees an interface that is added later.

use std::cell::Cell;
use std::ffi::CStr;
use std::mem::size_of;
use std::ptr;
use std::sync::Mutex;

use glib_sys::{GType, gpointer};
use gobject_sys::{
    GInterfaceInfo, GInterfaceInitFunc, GObject, GObjectClass, GTypeInfo, GTypeInstance,
    GTypeModule, GTypeQuery, g_type_add_interface_static, g_type_class_peek, g_type_class_ref,
    g_type_class_unref, g_type_default_interface_ref, g_type_default_interface_unref,
    g_type_from_name, g_type_get_plugin, g_type_module_add_interface, g_type_module_register_type,
    g_type_name, g_type_query, g_type_register_static,
};

use crate::trampoline::{guard, log_critical};

/// A type this crate can register.
///
/// # Safety
///
/// GObject reads and writes these structs with the layout that it gets from
/// `size_of`. Thus implementations must guarantee:
///
/// - `Instance` is `#[repr(C)]` and its first field is the parent type's
///   instance struct.
/// - `Class` is `#[repr(C)]` and its first field is the parent type's class
///   struct.
/// - [`parent_type`](ObjectSubclass::parent_type) returns the `GType` of
///   those two parent structs, and that type derives from `GObject`.
///   Registration overrides `GObjectClass.finalize` through the leading bytes
///   of `Class`.
///
/// An error in any of these moves every field and vfunc slot to an incorrect
/// position. The compiler cannot find this. `eds-sys`'s `g_type_query` layout
/// test prevents the same hazard on the C side.
pub unsafe trait ObjectSubclass: Sized {
    /// Must be unique in the process. GObject aborts on a duplicate.
    const NAME: &'static CStr;

    type Instance;
    type Class;

    fn parent_type() -> GType;

    /// The interfaces this type implements. Registration adds them to the
    /// type before it returns the `GType`.
    ///
    /// This is declared, and not done by the caller, because of timing. An
    /// interface must be on the type before its class is initialised. An
    /// implementer uses `g_object_class_override_property` to satisfy an
    /// interface's properties. It runs in
    /// [`class_init`](ObjectSubclass::class_init) and finds only properties of
    /// interfaces that the type already implements. If a caller added the
    /// interface after registration, its `GType` would implement nothing for
    /// some time. A reference to its class in that time would make the
    /// omission permanent.
    ///
    /// Each entry tells how the interface's vtable is filled:
    /// [`InterfaceDecl::defaults`] for one this type only claims, and
    /// [`InterfaceDecl::filled_by`] for one whose vfunc slots it fills.
    fn interfaces() -> Vec<InterfaceDecl> {
        Vec::new()
    }

    /// All of *our* types whose `GType` this type's
    /// [`class_init`](ObjectSubclass::class_init) asks for. They are
    /// registered before this type, so that the request cannot block.
    ///
    /// A class initialiser runs while it holds GLib's class-initialisation
    /// lock. Registration here is serialised by a lock of our own. Thus a
    /// `class_init` that registers holds GLib's lock and waits for ours. At
    /// the same time, a registration can hold ours and wait for GLib's, in
    /// `g_type_add_interface_static`. Every registration of a type that
    /// implements an interface ends there. That is a cycle. It stopped
    /// `jmap-mail`'s test binary for eleven minutes.
    ///
    /// Thus [`register_static`] does not take the lock when it is called in a
    /// class initialiser. It answers only from the type system, which works
    /// only when the type is already registered. A declaration here makes
    /// sure that the type is registered. The named accessors are called
    /// before this type's own registration starts, on a thread that holds
    /// neither lock.
    ///
    /// The list is for the *effect* of the accessors, not their values.
    /// Nothing reads the `GType`s. The type is `Vec<GType>`, not `()`, only so
    /// that an implementation looks like a list of types.
    ///
    /// Also declare here a type that is not ours: an Evolution or EDS class
    /// that `class_init` names, for example as an `extensible_type`. But
    /// `class_init` must never call its accessor, even when it is declared.
    /// See [`registered_type`] for the reason, and for what `class_init`
    /// calls.
    fn class_init_types() -> Vec<GType> {
        Vec::new()
    }

    /// The sizes at which GObject allocates this type's class and instance
    /// structs.
    ///
    /// The default is the sizes of the declared structs. This is correct when
    /// the parent's structs are generated and embedded as leading fields.
    /// This workspace intentionally knows some parents only as an opaque
    /// handle (a GTK widget, whose class structs the sys crates do not
    /// generate). A subclass of such a parent has no struct to embed. It
    /// overrides this with [`TypeSizes::of_parent`], adds no inline fields,
    /// and keeps its state in qdata. Then `Instance` and `Class` are only the
    /// types that the trampolines cast through, and their size does not
    /// matter. The leading-bytes contract still applies: both must *be*
    /// prefixes of the parent's structs. `GTypeInstance` and `GObjectClass`
    /// are the correct choices.
    fn type_sizes() -> TypeSizes {
        TypeSizes {
            class: size_of::<Self::Class>(),
            instance: size_of::<Self::Instance>(),
        }
    }

    /// Runs once, before any instance exists. Override vfunc slots in `Class`
    /// (and in the parent class through it) here.
    ///
    /// # Safety
    ///
    /// `class` points to a new class struct of this type, and the parent class
    /// is already initialised.
    unsafe fn class_init(_class: *mut Self::Class) {}

    /// Runs for each instance, after the parent's `instance_init`.
    ///
    /// # Safety
    ///
    /// `instance` points to a zeroed instance struct of this type.
    unsafe fn instance_init(_instance: *mut Self::Instance) {}

    /// Runs once per instance, when the last reference to it is dropped and
    /// before the parent's `finalize`. Destroy here all that
    /// [`instance_init`](ObjectSubclass::instance_init) created. GObject frees
    /// the instance struct without a Rust destructor. Thus a
    /// [`Slot`](crate::instance::Slot) that is not cleared here leaks its
    /// contents.
    ///
    /// Do not chain up here. Registration always does it.
    ///
    /// # Safety
    ///
    /// `instance` points to an instance struct of this type that is being
    /// finalized. No other code can get to it.
    unsafe fn finalize(_instance: *mut Self::Instance) {}
}

/// The return value of [`ObjectSubclass::type_sizes`]: the number of bytes
/// that GObject allocates for the class struct and for each instance.
pub struct TypeSizes {
    pub class: usize,
    pub instance: usize,
}

impl TypeSizes {
    /// The parent's own sizes, from the running type system. For a subclass
    /// that adds no inline fields. A subclass of an opaque-handle parent (a
    /// GTK widget) must have this shape, and keeps its state in qdata.
    ///
    /// The class is referenced first, because `g_type_query` fills nothing for
    /// a classed type whose class was never initialised. The layout tests of
    /// the sys crates do the same steps.
    pub fn of_parent(parent: GType) -> Self {
        // SAFETY: the caller uses `parent` as a registered classed type. The
        // ref is released after the query, and the class stays valid for the
        // process (the child registered against it keeps a reference).
        let query = unsafe {
            let class = g_type_class_ref(parent);
            assert!(!class.is_null(), "TypeSizes::of_parent: not a classed type");
            let mut query = std::mem::zeroed::<GTypeQuery>();
            g_type_query(parent, &mut query);
            g_type_class_unref(class);
            query
        };
        assert_ne!(query.type_, 0, "TypeSizes::of_parent: g_type_query failed");
        Self {
            class: query.class_size as usize,
            instance: query.instance_size as usize,
        }
    }
}

/// One interface that a type declares, and how to fill the copy of that
/// interface's vtable that GObject gives the type.
///
/// Use one of the two constructors, not the field names. The choice between
/// them is all the content of the type. If a caller could write `init: None`
/// for an interface with vfuncs, it would declare an implementation that calls
/// the value from the interface's `default_init`. For Camel's interfaces, that
/// value is a NULL, and the wrapper function calls it directly.
pub struct InterfaceDecl {
    gtype: GType,
    init: GInterfaceInitFunc,
}

impl InterfaceDecl {
    /// An interface that this type claims, but does not fill any slot of. The
    /// implementation is the value that the interface's `default_init` put in
    /// the vtable.
    ///
    /// Correct for an interface that has only properties. The implementer
    /// satisfies them in [`class_init`](ObjectSubclass::class_init) with
    /// `g_object_class_override_property`, not through the vtable.
    /// `CamelNetworkSettings`, the first interface for this hook, declares no
    /// vfuncs.
    pub fn defaults(gtype: GType) -> Self {
        Self { gtype, init: None }
    }

    /// An interface whose vtable `I` fills once, when GObject initialises this
    /// type's class.
    pub fn filled_by<I: InterfaceImpl>() -> Self {
        Self {
            gtype: I::gtype(),
            init: Some(interface_init_trampoline::<I>),
        }
    }
}

/// Fills one implementing type's copy of one interface's vtable. This is the
/// `G_IMPLEMENT_INTERFACE` init function, in Rust.
///
/// This is a separate trait. A type *next to* the class implements it, not
/// the class itself, because a class can implement many interfaces and then
/// needs many of these. The Camel provider's store already has that shape:
/// `CamelSubscribable` is one interface it fills, and there will be more.
///
/// # Why not from `class_init`
///
/// It is possible: `g_type_interface_peek` in
/// [`class_init`](ObjectSubclass::class_init) returns this type's own copy of
/// the vtable, and a slot written through it stays. This was tested, not
/// assumed, before this hook was written.
///
/// But it works only because of an order that GLib does not promise.
/// `gtype.c` base-initialises a type's interface vtables before it calls the
/// class's initialiser, and runs the `interface_init` functions after it. The
/// documentation does not say this. `g_type_interface_peek` is specified for a
/// type's interfaces, not for a class that is only partly built.
/// `GInterfaceInfo.interface_init` is the documented contract for this
/// purpose, so this crate uses it. Also, the implementer then gets a typed
/// `*mut Vtable` under a panic guard, and does not cast a `gpointer` at the
/// call site. The filling is declared next to the interface, not among the
/// class's own overrides.
///
/// # Safety
///
/// GObject writes through `Vtable` with the layout of the C interface struct.
/// Thus implementations must guarantee:
///
/// - `Vtable` is `#[repr(C)]`, is the binding of the interface struct
///   [`gtype`](InterfaceImpl::gtype) names, and leads with `GTypeInterface`.
/// - [`gtype`](InterfaceImpl::gtype) really is that interface's `GType`.
///
/// A mismatch puts a function pointer at the offset of a different slot. The
/// compiler cannot find this. The result is a call to the incorrect vfunc with
/// incorrect arguments.
pub unsafe trait InterfaceImpl {
    /// The interface's vtable struct (for example,
    /// `CamelSubscribableInterface`).
    type Vtable;

    /// The interface that this fills. A type accessor, not a constant, because
    /// a `GType` has a number only after the interface registers itself.
    fn gtype() -> GType;

    /// Writes this type's implementations into the slots it fills. The other
    /// slots keep the interface's own defaults.
    ///
    /// # Safety
    ///
    /// `vtable` points to the implementing type's own copy of the interface's
    /// vtable. It is newly initialised from the interface's defaults, and no
    /// other code can get to it yet.
    unsafe fn interface_init(vtable: *mut Self::Vtable);
}

/// GObject calls this in `g_type_class_ref`, while it holds the type system's
/// global lock. Thus it has the same guard as [`class_init_trampoline`]. A
/// panic that unwinds from here would abort the process, not only stop one
/// account.
///
/// After a caught panic, the vtable stays as far as the init got. A slot that
/// the init did not get to keeps the interface's own default. That is the
/// correct result. To put the defaults back, this code would have to copy a
/// vtable of unknown size.
unsafe extern "C" fn interface_init_trampoline<I: InterfaceImpl>(
    vtable: gpointer,
    _data: gpointer,
) {
    // SAFETY: the interface is registered (`filled_by` asked it for its
    // GType), so `g_type_name` returns its name, not NULL.
    let name = unsafe { CStr::from_ptr(g_type_name(I::gtype())) };
    let context = format!("{}::interface_init", name.to_string_lossy());

    inside_class_init(|| {
        guard(&context, (), || unsafe {
            I::interface_init(vtable.cast::<I::Vtable>())
        })
    });
}

/// A type that is not ours, found by name for a class initialiser.
///
/// The rule: no [`class_init`](ObjectSubclass::class_init) calls a foreign
/// `*_get_type()` accessor. It uses this function to find the type. The type
/// is named in [`class_init_types`](ObjectSubclass::class_init_types), so
/// that the lookup finds it.
///
/// The accessor causes the same cycle that `REGISTRATION` prevents, with
/// GLib's lazy `get_type` once-init in place of our lock. A class initialiser
/// holds GLib's class-init lock. An accessor whose type is not registered yet
/// waits in `g_once_init_enter` for the thread that registers it. That thread
/// registers a `GtkBox` or a different type that implements an interface. It
/// waits in `g_type_add_interface_static` for the class-init lock.
/// `jmap-config`'s `source_config` tests stopped because of this when one of
/// its declarations was missing. One thread initialised `EBookConfigJmap`'s
/// class and asked for `EBookSourceConfig`. A different thread registered
/// `ECalSourceConfig` and, under it, the `GtkBox` that both derive from.
///
/// A lookup takes no once-init, so it cannot wait on other threads. When the
/// type is not there, it returns 0 (`G_TYPE_INVALID`) and logs a critical
/// that names the missing declaration. `e_extensible_load_extensions` never
/// loads an extension whose class has an `extensible_type` of 0. A test then
/// fails in seconds. With the accessor, the full run stopped.
pub fn registered_type(name: &CStr) -> GType {
    // SAFETY: `name` is a NUL-terminated string that outlives the call.
    let gtype = unsafe { g_type_from_name(name.as_ptr()) };
    if gtype == 0 {
        log_critical(&format!(
            "{name:?}: asked for from a class initialiser before it was registered; \
             name it in the initialising type's class_init_types()"
        ));
    }
    gtype
}

/// Initialises the class of `T`'s parent and the default vtable of each
/// interface that `T` declares, on this thread, now. Only static types are
/// initialised. A static type keeps its class for the life of the process.
///
/// This prevents a deadlock in GLib. A foreign `class_init` often calls a
/// lazy `*_get_type()` (`EBackend`'s asks for `ESource`). Thread A holds
/// GLib's class-init lock in that `class_init` and waits in the accessor's
/// `g_once_init_enter`. Thread B owns that once-init. It registers a type
/// that implements an interface, and waits in `g_type_add_interface_static`
/// for the class-init lock.
///
/// [`register_static`] and [`register_dynamic`] call this before they
/// register `T`. In a module that is `e_module_load`, so the parents'
/// initialisers run there, and the first reference to our class runs only
/// our own `class_init`. A test binary starts many threads before it
/// registers anything. Thus use [`initialise_parents_before_main!`](crate::initialise_parents_before_main)
/// there as well.
pub fn initialise_parents<T: ObjectSubclass>() {
    initialise_foreign(T::parent_type(), &T::interfaces());
}

fn initialise_foreign(parent: GType, interfaces: &[InterfaceDecl]) {
    // SAFETY: `parent` and each interface come from type accessors. A static
    // type keeps a reference of its own, so the release keeps the class.
    unsafe {
        if g_type_get_plugin(parent).is_null() {
            g_type_class_unref(g_type_class_ref(parent));
        }
        for interface in interfaces {
            if g_type_get_plugin(interface.gtype).is_null() {
                g_type_default_interface_unref(g_type_default_interface_ref(interface.gtype));
            }
        }
    }
}

/// Calls [`initialise_parents`] for each named type before `main` starts.
///
/// Before `main`, a test binary has one thread, so no registration can run at
/// the same time. Use it once in each crate root, behind the crate's
/// `testing` feature. Name every type that the crate registers.
#[macro_export]
macro_rules! initialise_parents_before_main {
    ($($subclass:ty),+ $(,)?) => {
        const _: () = {
            extern "C" fn initialise() {
                $($crate::subclass::initialise_parents::<$subclass>();)+
            }

            #[used]
            #[unsafe(link_section = ".init_array")]
            static INITIALISE: extern "C" fn() = initialise;
        };
    };
}

/// The paths that [`test_module!`](crate::test_module) expands to.
#[cfg(feature = "testing")]
#[doc(hidden)]
pub mod __test_module {
    pub use glib_sys::{GTRUE, GType, gboolean};
    pub use gobject_sys::{GTypeModule, GTypeModuleClass};

    /// `G_TYPE_TYPE_MODULE`, resolved one time for the process.
    ///
    /// GLib's `g_type_module_get_type` is not thread-safe on its first call. It
    /// registers the type, then adds `GTypePlugin` in a second step. Tests run
    /// on parallel threads. Without the `OnceLock`, a second thread can derive a
    /// module before the interface is there, and its reload then fails.
    pub fn type_module_type() -> GType {
        static TYPE: std::sync::OnceLock<GType> = std::sync::OnceLock::new();
        // SAFETY: no arguments.
        *TYPE.get_or_init(|| unsafe { gobject_sys::g_type_module_get_type() })
    }
}

/// Defines `$name`, a `GTypeModule` stand-in for the `EModule` that EDS loads.
///
/// Its `load` vfunc calls `load` and returns `TRUE`. Its `unload` vfunc calls
/// `unload`. Without `unload`, the slot stays empty. Each function takes the
/// `*mut GTypeModule` that GLib passes, and can be safe or `unsafe`.
#[cfg(feature = "testing")]
#[macro_export]
macro_rules! test_module {
    ($name:ident, $type_name:expr, load = $load:path $(, unload = $unload:path)? $(,)?) => {
        #[repr(C)]
        struct $name {
            parent: $crate::subclass::__test_module::GTypeModule,
        }

        const _: () = {
            use $crate::subclass::__test_module::{
                GTRUE, GType, GTypeModule, GTypeModuleClass, gboolean, type_module_type,
            };

            #[repr(C)]
            struct Class {
                parent_class: GTypeModuleClass,
            }

            // SAFETY: both structs are #[repr(C)] and start with the GTypeModule
            // instance and class structs, and GTypeModule derives from GObject.
            unsafe impl $crate::subclass::ObjectSubclass for $name {
                const NAME: &'static ::std::ffi::CStr = $type_name;
                type Instance = $name;
                type Class = Class;

                fn parent_type() -> GType {
                    type_module_type()
                }

                unsafe fn class_init(class: *mut Self::Class) {
                    #[allow(unused_unsafe)]
                    unsafe extern "C" fn module_load(module: *mut GTypeModule) -> gboolean {
                        // SAFETY: GLib passes the module that it loads.
                        unsafe { $load(module) };
                        GTRUE
                    }

                    // SAFETY: `class` points to an allocated class struct that
                    // starts with GTypeModuleClass.
                    let vfuncs = unsafe { &mut (*class).parent_class };
                    vfuncs.load = Some(module_load);
                    $(
                        #[allow(unused_unsafe)]
                        unsafe extern "C" fn module_unload(module: *mut GTypeModule) {
                            // SAFETY: GLib passes the module that it unloads.
                            unsafe { $unload(module) };
                        }

                        vfuncs.unload = Some(module_unload);
                    )?
                }
            }
        };
    };
}

/// Registers `T` as a static type, or returns the existing `GType` if it is
/// already registered.
///
/// Idempotence is necessary. A second `g_type_register_static` with the same
/// name is a fatal GLib error, and a process can do the same registration two
/// times (a test suite from two threads, a Camel provider from two
/// `camel_provider_module_init`s).
pub fn register_static<T: ObjectSubclass>() -> GType {
    // SAFETY: a null GTypeModule selects the static path, which has no
    // pointer requirements.
    unsafe { register::<T>(ptr::null_mut()) }
}

/// Registers `T` against the `GTypeModule` that EDS loaded us as. Thus the
/// type is unregistered if the module is unloaded.
///
/// Call this on *every* load of the module, not only the first. When a module
/// is unused, GLib marks its registered types as unloaded. GLib does not
/// return such a type until a later load registers it again.
///
/// # Safety
///
/// `module` must be a valid `GTypeModule`, i.e. the pointer EDS passed to
/// `e_module_load`.
pub unsafe fn register_dynamic<T: ObjectSubclass>(module: *mut GTypeModule) -> GType {
    debug_assert!(!module.is_null(), "register_dynamic needs a GTypeModule");
    unsafe { register::<T>(module) }
}

/// `class_type`'s class, as the `C` that it starts with. For a vfunc that
/// chains up to a class that this crate does not own.
///
/// `g_type_class_peek`, not `_ref`. The registration of `class_type` (or a
/// subclass of it) already guarantees an initialised class. A reference here
/// would need a release on a path that has no natural place for it.
///
/// # Safety
///
/// `class_type`'s class struct must start with a `C`. An instance of
/// `class_type` or one of its subclasses must already exist. This guarantees
/// that the class is initialised. Then it stays alive for the life of the
/// process.
pub unsafe fn parent_class<C>(class_type: GType) -> Option<&'static C> {
    // SAFETY: the contract above.
    unsafe { g_type_class_peek(class_type).cast::<C>().as_ref() }
}

/// Serialises the check-then-register pair. GObject's own registration is
/// thread-safe, but "is it there yet?" then "register it" is not. If a thread
/// loses that race, the process aborts.
///
/// Taken *before* GLib's class-initialisation lock and never after it.
/// [`IN_CLASS_INIT`] makes sure that it is never taken after it.
static REGISTRATION: Mutex<()> = Mutex::new(());

thread_local! {
    /// The number of class or interface initialisers that this thread is in.
    ///
    /// GLib holds its class-initialisation lock across both. If an
    /// initialiser took [`REGISTRATION`], the lock order would be the opposite
    /// of a registration that already holds the locks the other way. A
    /// non-zero value means that [`register`] must answer without the lock, or
    /// not at all. See [`ObjectSubclass::class_init_types`] for the full cycle.
    ///
    /// A counter, not a flag, because the initialisers nest. A reference to a
    /// class from in one runs that class's initialiser on the same thread.
    static IN_CLASS_INIT: Cell<u32> = const { Cell::new(0) };
}

/// Runs `f` with [`IN_CLASS_INIT`] increased, and always decreases it after.
/// A panic here unwinds only to the guard inside. But if the count stayed
/// high, registration on this thread would silently stop for the rest of
/// the process.
fn inside_class_init<T>(f: impl FnOnce() -> T) -> T {
    struct Lower;
    impl Drop for Lower {
        fn drop(&mut self) {
            IN_CLASS_INIT.with(|depth| depth.set(depth.get().saturating_sub(1)));
        }
    }

    IN_CLASS_INIT.with(|depth| depth.set(depth.get() + 1));
    let _lower = Lower;
    f()
}

unsafe fn register<T: ObjectSubclass>(module: *mut GTypeModule) -> GType {
    // A class initialiser asks for a type that it declared, under GLib's
    // lock. Answer from the type system, and take no lock. `class_init_types`
    // exists so that this registration is already done. Thus a miss means
    // that the declaration is missing. Report it and do not hide it, because
    // no code can build from the zero GType that this returns.
    if IN_CLASS_INIT.with(Cell::get) > 0 {
        return registered_type(T::NAME);
    }

    // Resolved before the lock is taken, not in the GTypeInfo below. A
    // parent_type() can register a different one of our types. (A hierarchy
    // declared fully in Rust bootstraps itself this way.) Otherwise it would
    // deadlock on this non-reentrant mutex. The same applies to the
    // interfaces, which are resolved through their own type accessors. It
    // also applies to the types that class_init needs. They are resolved here
    // for their effect, not for the GTypes they return.
    let parent = T::parent_type();
    let interfaces = T::interfaces();
    let sizes = T::type_sizes();
    let _class_init_types = T::class_init_types();
    initialise_foreign(parent, &interfaces);

    // A poisoned lock means that a different registration panicked. That
    // does not change the type system, so continue and do not panic.
    let _guard = REGISTRATION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // Only on the static path. A second `g_type_register_static` with the
    // same name is a fatal GLib error. Thus return an already-registered type
    // immediately. `g_type_module_register_type` is the opposite. You *must*
    // call it again on every load of the module. When a module is unused,
    // GLib marks every type it registered as unloaded. GLib then refuses the
    // module until a second registration marks them as loaded again. When
    // code asks for one of those types, GLib aborts the process ("Could not
    // reload previously loaded plugin").
    if module.is_null() {
        // SAFETY: NAME is a 'static NUL-terminated string.
        let existing = unsafe { g_type_from_name(T::NAME.as_ptr()) };
        if existing != 0 {
            return existing;
        }
    }

    let class_size = u16::try_from(sizes.class)
        .unwrap_or_else(|_| panic!("{:?}: class struct exceeds GObject's 64 KiB", T::NAME));
    let instance_size = u16::try_from(sizes.instance)
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
    // contract fixes to the parent's. It must only outlive the call.
    let gtype = unsafe {
        if module.is_null() {
            g_type_register_static(parent, T::NAME.as_ptr(), &info, 0)
        } else {
            g_type_module_register_type(module, parent, T::NAME.as_ptr(), &info, 0)
        }
    };

    // Do this before the type is returned, thus before any code can reference
    // its class. If an interface is added after class_init runs, the class
    // never overrides its properties.
    for interface in interfaces {
        let interface_info = GInterfaceInfo {
            interface_init: interface.init,
            interface_finalize: None,
            interface_data: ptr::null_mut(),
        };

        // SAFETY: `gtype` is newly registered, and `interface.gtype` came from
        // a type accessor. `interface_info` outlives both calls. On the static
        // path the type is new, so the interface cannot already be on it. On
        // the dynamic path, the documentation says to call
        // `g_type_module_add_interface` again on every load, as for the
        // registration above.
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

/// GObject calls these from C, so they have guards. Without a guard, a panic
/// in a user `class_init` would unwind through the type system's own frames
/// while the type system holds its global lock.
unsafe extern "C" fn class_init_trampoline<T: ObjectSubclass>(class: gpointer, _data: gpointer) {
    // Installed before `T::class_init` runs, so a subclass can still take the
    // slot for itself. Always installed, because for a type with nothing to
    // destroy, the cost is only an empty call.
    //
    // SAFETY: `class` starts with the parent's class struct. The trait's
    // contract requires that it derives from GObjectClass.
    unsafe { (*class.cast::<GObjectClass>()).finalize = Some(finalize_trampoline::<T>) };

    inside_class_init(|| {
        guard("class_init", (), || unsafe {
            T::class_init(class.cast::<T::Class>())
        })
    });
}

/// Drops what `instance_init` created, then gives the instance to the
/// parent. The parent frees it.
///
/// The chain-up is intentionally outside the guard. A panic in `T::finalize`
/// is already a bug. If the parent's finalize did not run, every instance
/// after that would leak. For an `EBookMetaBackend`, this includes the
/// `ESource` and the offline cache.
unsafe extern "C" fn finalize_trampoline<T: ObjectSubclass>(object: *mut GObject) {
    guard("finalize", (), || unsafe {
        T::finalize(object.cast::<T::Instance>())
    });

    // Use the parent class, not `g_type_class_peek_parent` of the instance's
    // class. For a further subclass of ours, that would point back to this
    // trampoline and recurse until the stack overflows.
    //
    // SAFETY: an instance of this type exists, so its class exists. Thus the
    // class it derives from is initialised and alive.
    let parent = unsafe { g_type_class_peek(T::parent_type()) }.cast::<GObjectClass>();
    match unsafe { parent.as_ref() }.and_then(|class| class.finalize) {
        // SAFETY: this is the parent's own finalize, called on an instance of
        // a type derived from it. This is how C chains up.
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use gobject_sys::{G_TYPE_OBJECT, g_type_class_ref, g_type_class_unref};
    use jmap_tracing_capture::capture;

    use super::*;

    /// The shape of a GTK-widget subclass: sizes from the running type
    /// system, not from declared structs, and no inline fields of its own.
    struct QueriedSizes;

    // SAFETY: GTypeInstance and GObjectClass are prefixes of G_TYPE_OBJECT's
    // structs, and type_sizes() returns that parent's real sizes.
    unsafe impl ObjectSubclass for QueriedSizes {
        const NAME: &'static CStr = c"JmapBackendCoreQueriedSizes";
        type Instance = GTypeInstance;
        type Class = GObjectClass;

        fn parent_type() -> GType {
            G_TYPE_OBJECT
        }

        fn type_sizes() -> TypeSizes {
            TypeSizes::of_parent(Self::parent_type())
        }
    }

    /// `of_parent` returns the parent's own sizes, and a type registered with
    /// them is allocated at those sizes. The GtkBox page in jmap-ui needs this
    /// contract.
    #[test]
    fn queried_sizes_match_the_parent() {
        let sizes = TypeSizes::of_parent(G_TYPE_OBJECT);
        assert_eq!(sizes.class, size_of::<GObjectClass>());
        assert_eq!(sizes.instance, size_of::<GObject>());

        let gtype = register_static::<QueriedSizes>();
        // SAFETY: `gtype` is newly registered, and the class ref is released.
        let query = unsafe {
            let class = g_type_class_ref(gtype);
            let mut query = std::mem::zeroed::<GTypeQuery>();
            g_type_query(gtype, &mut query);
            g_type_class_unref(class);
            query
        };
        assert_eq!(query.instance_size as usize, sizes.instance);
        assert_eq!(query.class_size as usize, sizes.class);
    }

    /// An empty type. It is registered so that a different type's
    /// `class_init` can ask for its `GType`.
    struct Dependency;

    // SAFETY: GObject/GObjectClass are the parent structs of G_TYPE_OBJECT,
    // not extended. This satisfies the "leads with the parent" contract.
    unsafe impl ObjectSubclass for Dependency {
        const NAME: &'static CStr = c"JmapBackendCoreLockOrderDependency";
        type Instance = GObject;
        type Class = GObjectClass;

        fn parent_type() -> GType {
            G_TYPE_OBJECT
        }
    }

    /// What [`Dependent::class_init`] got when it asked for the dependency's
    /// `GType`. Zero means that it did not ask.
    static SEEN_FROM_CLASS_INIT: AtomicUsize = AtomicUsize::new(0);

    /// The shape of `CamelJmapStore` and `CamelJmapTransport`: a class
    /// initialiser that needs the `GType` of a different one of our types.
    /// They write it into `CamelServiceClass.settings_type`.
    struct Dependent;

    // SAFETY: as `Dependency`.
    unsafe impl ObjectSubclass for Dependent {
        const NAME: &'static CStr = c"JmapBackendCoreLockOrderDependent";
        type Instance = GObject;
        type Class = GObjectClass;

        fn parent_type() -> GType {
            G_TYPE_OBJECT
        }

        unsafe fn class_init(_class: *mut Self::Class) {
            SEEN_FROM_CLASS_INIT.store(register_static::<Dependency>(), Ordering::SeqCst);
        }
    }

    /// The class-init/registration-lock deadlock first found in
    /// `jmap-mail`'s `tests/transport.rs`, reduced to the part this crate owns.
    ///
    /// GLib serialises class initialisation with its own recursive lock, and
    /// holds it across `class_init`. If that `class_init` then waits for
    /// [`REGISTRATION`], GLib's lock waits for ours. At the same time, a
    /// registration that holds ours waits for GLib's, in
    /// `g_type_add_interface_static`. That is a two-thread cycle that never
    /// ends.
    ///
    /// Here, the test holds `REGISTRATION` in place of that other thread. A
    /// `class_init` must complete without it, whatever else registers.
    #[test]
    fn a_class_init_does_not_wait_for_the_registration_lock() {
        let dependent = register_static::<Dependent>();
        let dependency = register_static::<Dependency>();

        let held = REGISTRATION
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let (done, ran) = mpsc::channel();
        let initialiser = thread::spawn(move || {
            // SAFETY: `dependent` is the newly registered GType. The class is
            // released below.
            let class = unsafe { g_type_class_ref(dependent) };
            let _ = done.send(());
            unsafe { g_type_class_unref(class) };
        });

        let outcome = ran.recv_timeout(Duration::from_secs(10));
        // Always released, so a failing test still joins and does not stop
        // the run.
        drop(held);
        initialiser.join().expect("the class_init thread panicked");

        assert!(
            outcome.is_ok(),
            "class_init blocked while another registration held REGISTRATION"
        );
        assert_eq!(
            SEEN_FROM_CLASS_INIT.load(Ordering::SeqCst),
            dependency,
            "class_init did not get the dependency's GType"
        );
    }

    /// A dependency that nothing registers before, so only the declaration
    /// below can register it.
    struct DeclaredDependency;

    // SAFETY: as `Dependency`.
    unsafe impl ObjectSubclass for DeclaredDependency {
        const NAME: &'static CStr = c"JmapBackendCoreDeclaredDependency";
        type Instance = GObject;
        type Class = GObjectClass;

        fn parent_type() -> GType {
            G_TYPE_OBJECT
        }
    }

    static DECLARED_FROM_CLASS_INIT: AtomicUsize = AtomicUsize::new(0);

    struct Declaring;

    // SAFETY: as `Dependency`.
    unsafe impl ObjectSubclass for Declaring {
        const NAME: &'static CStr = c"JmapBackendCoreDeclaringType";
        type Instance = GObject;
        type Class = GObjectClass;

        fn parent_type() -> GType {
            G_TYPE_OBJECT
        }

        fn class_init_types() -> Vec<GType> {
            vec![register_static::<DeclaredDependency>()]
        }

        unsafe fn class_init(_class: *mut Self::Class) {
            DECLARED_FROM_CLASS_INIT
                .store(register_static::<DeclaredDependency>(), Ordering::SeqCst);
        }
    }

    /// The other part of the fix: `class_init` can answer only from the type
    /// system, so the type must be in it. The declaration puts it there. This
    /// occurs on the thread that registers the declaring type, before that
    /// registration starts.
    #[test]
    fn a_declared_type_is_registered_before_the_type_that_declares_it() {
        let declaring = register_static::<Declaring>();

        // SAFETY: `declaring` is the newly registered GType.
        let class = unsafe { g_type_class_ref(declaring) };
        unsafe { g_type_class_unref(class) };

        let dependency = DECLARED_FROM_CLASS_INIT.load(Ordering::SeqCst);
        assert_ne!(dependency, 0, "class_init got no GType for its dependency");
        // SAFETY: NAME is a 'static NUL-terminated string.
        assert_eq!(dependency, unsafe {
            g_type_from_name(DeclaredDependency::NAME.as_ptr())
        });
    }

    /// After a foreign type is registered, the lookup finds it by name. It
    /// calls no accessor, so there is no once-init to wait on.
    #[test]
    fn a_registered_type_is_looked_up_by_name() {
        assert_eq!(registered_type(c"GObject"), G_TYPE_OBJECT);
    }

    /// A miss gives 0 (`G_TYPE_INVALID`): an `extensible_type` that matches
    /// nothing, not a wait that never ends.
    #[test]
    fn an_unregistered_type_is_looked_up_as_invalid() {
        assert_eq!(registered_type(c"JmapBackendCoreNeverRegistered"), 0);
    }

    /// A miss also logs a critical that names the type. Thus a developer
    /// sees why an extension did not load, not only a silent `GType` of 0.
    #[test]
    fn an_unregistered_type_logs_a_critical_naming_it() {
        let (_, captured) = capture(|| {
            registered_type(c"JmapBackendCoreNeverRegistered");
        });

        assert!(
            captured
                .iter()
                .any(|(level, _, value)| *level == tracing::Level::ERROR
                    && value.contains("JmapBackendCoreNeverRegistered")),
            "expected an ERROR event naming the missing type, got {captured:?}"
        );
    }

    /// A hit logs nothing. The lookup succeeded, so there is no bug to report.
    #[test]
    fn a_registered_type_logs_nothing() {
        let (_, captured) = capture(|| {
            registered_type(c"GObject");
        });

        assert!(
            captured.is_empty(),
            "expected no log event for a successful lookup, got {captured:?}"
        );
    }

    static UNDECLARED_FROM_CLASS_INIT: AtomicUsize = AtomicUsize::new(1);

    struct UndeclaredDependency;

    // SAFETY: as `Dependency`.
    unsafe impl ObjectSubclass for UndeclaredDependency {
        const NAME: &'static CStr = c"JmapBackendCoreUndeclaredDependency";
        type Instance = GObject;
        type Class = GObjectClass;

        fn parent_type() -> GType {
            G_TYPE_OBJECT
        }
    }

    struct Undeclaring;

    // SAFETY: as `Dependency`.
    unsafe impl ObjectSubclass for Undeclaring {
        const NAME: &'static CStr = c"JmapBackendCoreUndeclaringType";
        type Instance = GObject;
        type Class = GObjectClass;

        fn parent_type() -> GType {
            G_TYPE_OBJECT
        }

        unsafe fn class_init(_class: *mut Self::Class) {
            UNDECLARED_FROM_CLASS_INIT
                .store(register_static::<UndeclaredDependency>(), Ordering::SeqCst);
        }
    }

    /// The result of the refusal when the declaration is missing: zero and a
    /// GLib critical, not a registration under GLib's lock. Zero is a bad
    /// `GType`, and GLib then tells the caller about it. That gives a bug
    /// report. The deadlock gave a hang with nothing in the log.
    #[test]
    fn an_undeclared_type_is_refused_rather_than_registered_from_a_class_init() {
        let undeclaring = register_static::<Undeclaring>();

        // SAFETY: `undeclaring` is the newly registered GType.
        let class = unsafe { g_type_class_ref(undeclaring) };
        unsafe { g_type_class_unref(class) };

        assert_eq!(
            UNDECLARED_FROM_CLASS_INIT.load(Ordering::SeqCst),
            0,
            "class_init was handed a GType for a type nothing had registered"
        );
        // SAFETY: NAME is a 'static NUL-terminated string.
        assert_eq!(
            unsafe { g_type_from_name(UndeclaredDependency::NAME.as_ptr()) },
            0,
            "the type was registered anyway, from under GLib's lock"
        );
    }

    /// What the parent's `class_init` saw: the child's `GType` when it ran,
    /// plus one. Zero means that it did not run.
    static CHILD_SEEN_BY_PARENT: AtomicUsize = AtomicUsize::new(0);

    /// The same for the interface's `default_init`.
    static CHILD_SEEN_BY_INTERFACE: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn fresh_parent_class_init(_class: gpointer, _data: gpointer) {
        // SAFETY: NAME is a 'static NUL-terminated string.
        let child = unsafe { g_type_from_name(ChildOfFresh::NAME.as_ptr()) };
        CHILD_SEEN_BY_PARENT.store(child + 1, Ordering::SeqCst);
    }

    unsafe extern "C" fn fresh_interface_default_init(_vtable: gpointer, _data: gpointer) {
        // SAFETY: as above.
        let child = unsafe { g_type_from_name(ChildOfFresh::NAME.as_ptr()) };
        CHILD_SEEN_BY_INTERFACE.store(child + 1, Ordering::SeqCst);
    }

    /// A static parent whose class nothing initialises before the child is
    /// registered. It stands in for an EDS parent such as `ECalMetaBackend`.
    fn fresh_parent() -> GType {
        static PARENT: std::sync::OnceLock<GType> = std::sync::OnceLock::new();
        // SAFETY: the sizes are those of GObject's own structs, and the
        // initialiser matches GClassInitFunc.
        *PARENT.get_or_init(|| unsafe {
            gobject_sys::g_type_register_static_simple(
                G_TYPE_OBJECT,
                c"JmapBackendCoreFreshParent".as_ptr(),
                size_of::<GObjectClass>() as u32,
                Some(fresh_parent_class_init),
                size_of::<GObject>() as u32,
                None,
                0,
            )
        })
    }

    /// A static interface whose default vtable nothing initialises first.
    fn fresh_interface() -> GType {
        static INTERFACE: std::sync::OnceLock<GType> = std::sync::OnceLock::new();
        // SAFETY: a GTypeInterface-sized vtable, and an initialiser that
        // matches GClassInitFunc.
        *INTERFACE.get_or_init(|| unsafe {
            gobject_sys::g_type_register_static_simple(
                gobject_sys::G_TYPE_INTERFACE,
                c"JmapBackendCoreFreshInterface".as_ptr(),
                size_of::<gobject_sys::GTypeInterface>() as u32,
                Some(fresh_interface_default_init),
                0,
                None,
                0,
            )
        })
    }

    struct ChildOfFresh;

    // SAFETY: the parent's structs are GObject's own, not extended.
    unsafe impl ObjectSubclass for ChildOfFresh {
        const NAME: &'static CStr = c"JmapBackendCoreChildOfFresh";
        type Instance = GObject;
        type Class = GObjectClass;

        fn parent_type() -> GType {
            fresh_parent()
        }

        fn interfaces() -> Vec<InterfaceDecl> {
            vec![InterfaceDecl::defaults(fresh_interface())]
        }
    }

    /// The class-init deadlock of `jmap-backend-cal`'s `tests/scheduling.rs`.
    /// `EBackend`'s `class_init` asks for `ESource`. It ran on the first
    /// reference to our class, on any thread. Registration now runs it, on
    /// the registering thread, before our type exists. In a module, that is
    /// `e_module_load`.
    #[test]
    fn registration_initialises_the_parent_class_and_interfaces_first() {
        let child = register_static::<ChildOfFresh>();

        assert_eq!(
            CHILD_SEEN_BY_PARENT.load(Ordering::SeqCst),
            1,
            "the parent's class_init did not run before the child was registered"
        );
        assert_eq!(
            CHILD_SEEN_BY_INTERFACE.load(Ordering::SeqCst),
            1,
            "the interface's default_init did not run before the child was registered"
        );
        // SAFETY: registered static types.
        unsafe {
            assert!(
                !g_type_class_peek(fresh_parent()).is_null(),
                "the parent class was not kept"
            );
            assert!(
                !gobject_sys::g_type_default_interface_peek(fresh_interface()).is_null(),
                "the default vtable was not kept"
            );
            assert!(
                g_type_class_peek(child).is_null(),
                "the child's own class was initialised"
            );
        }
    }
}
