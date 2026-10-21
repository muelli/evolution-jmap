// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The due time of a deferred collection delete: a custom `ESourceExtension`,
//! `[JMAP Pending Delete]` in the keyfile of a child source.
//!
//! [`jmap_collection_sync::pending`] tells why a delete waits for an undo
//! window. The registry writes the due time here when the user deletes a
//! collection. Evolution reads it to show the info bar with "Undo". Thus both
//! processes register this type at module load. The due time must stay after a
//! restart of the registry, so it is a property with `E_SOURCE_PARAM_SETTING`.
//! [`crate::oauth2`] tells why only such a property gets to the `.source` file.

use std::ffi::CStr;
use std::ptr;
use std::sync::atomic::{AtomicI64, Ordering};

use eds_sys::{
    ESource, ESourceExtension, ESourceExtensionClass, e_source_extension_get_type,
    e_source_get_extension,
};
use glib_sys::GType;
use gobject_sys::{
    G_PARAM_READWRITE, G_PARAM_USER_SHIFT, G_TYPE_INT64, GObject, GObjectClass, GParamFlags,
    GParamSpec, GValue, g_object_class_install_property, g_object_get_property,
    g_object_set_property, g_param_spec_int64, g_value_get_int64, g_value_init, g_value_set_int64,
    g_value_unset,
};
use jmap_backend_core::instance::Slot;
use jmap_backend_core::marshal::extension_if_present;
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_backend_core::trampoline::{guard, log_critical};
use jmap_collection_sync::pending::Due;

/// The keyfile group name, and the `extension_name` of `e_source_get_extension`.
pub const EXTENSION_NAME: &CStr = c"JMAP Pending Delete";

/// The property that holds the due time: seconds since the Unix epoch, or zero
/// for no pending delete. The keyfile key is `Due`.
pub const PROPERTY_DUE: &CStr = c"due";

const PROP_DUE: u32 = 1;

/// `E_SOURCE_PARAM_SETTING`. [`crate::oauth2`] tells why it is calculated here.
const E_SOURCE_PARAM_SETTING: GParamFlags = 1 << (G_PARAM_USER_SHIFT as u32);

/// The instance struct: [`ESourceExtension`]'s own state plus the due time.
#[repr(C)]
pub struct Extension {
    parent: ESourceExtension,
    due: Slot<AtomicI64>,
}

/// The class struct: only [`ESourceExtensionClass`]'s own state.
#[repr(C)]
pub struct ExtensionClass {
    parent_class: ESourceExtensionClass,
}

// SAFETY: both structs are #[repr(C)] and start with ESourceExtension's own
// instance and class structs. ESourceExtension derives from GObject.
unsafe impl ObjectSubclass for Extension {
    const NAME: &'static CStr = c"JmapPendingDeleteExtension";
    type Instance = Extension;
    type Class = ExtensionClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_source_extension_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` starts with `ESourceExtensionClass`. The name lookup
        // of `e_source_get_extension` reads this field.
        unsafe { (*class).parent_class.name = EXTENSION_NAME.as_ptr() };

        // SAFETY: `ExtensionClass` starts with `ESourceExtensionClass`, which
        // starts with `GObjectClass`.
        let object_class = class.cast::<GObjectClass>();
        unsafe {
            (*object_class).set_property = Some(set_property);
            (*object_class).get_property = Some(get_property);
            g_object_class_install_property(
                object_class,
                PROP_DUE,
                g_param_spec_int64(
                    PROPERTY_DUE.as_ptr(),
                    ptr::null(),
                    ptr::null(),
                    i64::MIN,
                    i64::MAX,
                    0,
                    G_PARAM_READWRITE | E_SOURCE_PARAM_SETTING,
                ),
            );
        }
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // SAFETY: the instance is newly zeroed, which is the empty state of the slot.
        unsafe { (*instance).due.init(AtomicI64::new(0)) };
    }

    unsafe fn finalize(instance: *mut Self::Instance) {
        // SAFETY: the last reference is released, so nothing else can reach `due`.
        unsafe { (*instance).due.clear() };
    }
}

/// Returns the due time storage of `object`.
///
/// # Safety
///
/// `object` must be a live instance of [`Extension`] whose `instance_init` ran.
unsafe fn due_of<'a>(object: *mut GObject) -> &'a AtomicI64 {
    // SAFETY: the contract of the caller.
    unsafe { &*object.cast::<Extension>() }
        .due
        .get()
        .expect("a property vfunc dispatched before instance_init ran")
}

/// Writes the property. The EDS `.source` parser calls this through `g_object_set`.
unsafe extern "C" fn set_property(
    object: *mut GObject,
    id: u32,
    value: *mut GValue,
    _pspec: *mut GParamSpec,
) {
    guard("JmapPendingDeleteExtension::set_property", (), || {
        if id != PROP_DUE {
            log_critical(&format!(
                "JmapPendingDeleteExtension has no property with id {id}; the value is dropped"
            ));
            return;
        }
        // SAFETY: GObject passes an instance of this type, and `value` holds
        // the int64 that the pspec declares.
        unsafe { due_of(object).store(g_value_get_int64(value), Ordering::Relaxed) };
    });
}

/// Reads the property. This is the opposite of [`set_property`].
unsafe extern "C" fn get_property(
    object: *mut GObject,
    id: u32,
    value: *mut GValue,
    _pspec: *mut GParamSpec,
) {
    guard("JmapPendingDeleteExtension::get_property", (), || {
        if id != PROP_DUE {
            log_critical(&format!(
                "JmapPendingDeleteExtension has no property with id {id}; nothing is read"
            ));
            return;
        }
        // SAFETY: as in `set_property`. `value` is the int64 `GValue` to fill.
        unsafe { g_value_set_int64(value, due_of(object).load(Ordering::Relaxed)) };
    });
}

/// Registers [`Extension`], or finds the existing registration.
///
/// The module load of each process that parses a JMAP child source must call
/// this. EDS ignores a keyfile group with no registered type, so without this
/// call a restart loses the due time. [`read`] and [`write`] also call it.
pub fn ensure_registered() {
    jmap_backend_core::subclass::register_static::<Extension>();
}

/// Returns the due time of `source`, or `None` if no delete is pending.
///
/// A read does not add the extension to the source.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn read(source: *mut ESource) -> Option<Due> {
    ensure_registered();
    // SAFETY: `source` is valid by the contract, and the type is registered.
    let extension = unsafe { extension_if_present::<GObject>(source, EXTENSION_NAME) }?;
    // SAFETY: a live extension of this class, with an int64 `due` property.
    // The GValue is initialised before the read and unset after it.
    let stamp = unsafe {
        let mut gvalue: GValue = std::mem::zeroed();
        g_value_init(&mut gvalue, G_TYPE_INT64);
        g_object_get_property(extension, PROPERTY_DUE.as_ptr(), &mut gvalue);
        let stamp = g_value_get_int64(&gvalue);
        g_value_unset(&mut gvalue);
        stamp
    };
    Due::from_stamp(stamp)
}

/// Writes `due` to `source`. `None` clears a pending delete.
///
/// The write goes through the GObject property, so it emits `notify`. The
/// registry sends the changed source to Evolution on that signal. A clear on a
/// source without the extension does nothing and does not add the extension.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn write(source: *mut ESource, due: Option<Due>) {
    ensure_registered();
    // SAFETY: `source` is valid by the contract, and the type is registered.
    let extension = match due {
        Some(_) => unsafe { e_source_get_extension(source, EXTENSION_NAME.as_ptr()) }.cast(),
        None => match unsafe { extension_if_present::<GObject>(source, EXTENSION_NAME) } {
            Some(extension) => extension,
            None => return,
        },
    };
    // SAFETY: a live extension of this class, with an int64 `due` property.
    // The GValue is initialised before the write and unset after it.
    unsafe {
        let mut gvalue: GValue = std::mem::zeroed();
        g_value_init(&mut gvalue, G_TYPE_INT64);
        g_value_set_int64(&mut gvalue, due.map_or(0, Due::stamp));
        g_object_set_property(extension, PROPERTY_DUE.as_ptr(), &gvalue);
        g_value_unset(&mut gvalue);
    }
}
