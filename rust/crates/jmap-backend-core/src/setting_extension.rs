// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A custom `ESourceExtension` with one setting property, for example
//! `[JMAP Rebase]` in [`crate::rebase`].
//!
//! The property has `E_SOURCE_PARAM_SETTING`, so the (de)serialisation of
//! `ESource` reads and writes it in the `.source` file. Each [`Setting`]
//! gives one registered type, [`SettingExtension`] of that setting.
//! `jmap_config::oauth2` tells why a custom extension is necessary.

use std::ffi::CStr;
use std::marker::PhantomData;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use eds_sys::{
    E_SOURCE_PARAM_SETTING, ESource, ESourceExtension, ESourceExtensionClass,
    e_source_extension_get_type, e_source_get_extension,
};
use glib_sys::{GFALSE, GType};
use gobject_sys::{
    G_PARAM_READWRITE, G_TYPE_BOOLEAN, G_TYPE_INT64, GObject, GObjectClass, GParamSpec, GValue,
    g_object_class_install_property, g_object_set_property, g_param_spec_boolean,
    g_param_spec_int64, g_value_get_boolean, g_value_get_int64, g_value_init, g_value_set_boolean,
    g_value_set_int64, g_value_unset,
};

use crate::marshal::extension_if_present;
use crate::subclass::{ObjectSubclass, register_static};
use crate::trampoline::{guard, log_critical};

/// One account setting, stored as an `ESourceExtension` with one property.
pub trait Setting: 'static {
    /// The `GType` name of the extension class.
    const TYPE_NAME: &'static CStr;
    /// The keyfile group, and the `extension_name` of `e_source_get_extension`.
    const EXTENSION_NAME: &'static CStr;
    /// The property name. EDS makes the keyfile key from it.
    const PROPERTY: &'static CStr;
    /// The type of the property: `bool` or `i64`.
    type Value: SettingValue;
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for bool {}
    impl Sealed for i64 {}
}

/// A property type that [`SettingExtension`] can store.
pub trait SettingValue: Copy + sealed::Sealed + 'static {
    /// The storage in the instance. Its zeroed bit pattern is the default.
    #[doc(hidden)]
    type Cell: Send + Sync;
    #[doc(hidden)]
    const GTYPE: GType;
    #[doc(hidden)]
    fn new_cell() -> Self::Cell;
    #[doc(hidden)]
    fn load(cell: &Self::Cell) -> Self;
    #[doc(hidden)]
    fn store(cell: &Self::Cell, value: Self);
    /// # Safety
    ///
    /// `name` must stay alive for the life of the process.
    #[doc(hidden)]
    unsafe fn param_spec(name: &CStr) -> *mut GParamSpec;
    /// # Safety
    ///
    /// `value` must be an initialised `GValue` of [`Self::GTYPE`].
    #[doc(hidden)]
    unsafe fn from_gvalue(value: *const GValue) -> Self;
    /// # Safety
    ///
    /// `value` must be an initialised `GValue` of [`Self::GTYPE`].
    #[doc(hidden)]
    unsafe fn to_gvalue(self, value: *mut GValue);
}

impl SettingValue for bool {
    type Cell = AtomicBool;
    const GTYPE: GType = G_TYPE_BOOLEAN;

    fn new_cell() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn load(cell: &AtomicBool) -> bool {
        cell.load(Ordering::Relaxed)
    }

    fn store(cell: &AtomicBool, value: bool) {
        cell.store(value, Ordering::Relaxed);
    }

    unsafe fn param_spec(name: &CStr) -> *mut GParamSpec {
        // SAFETY: `name` lives for the process, by the contract of the caller.
        unsafe {
            g_param_spec_boolean(
                name.as_ptr(),
                ptr::null(),
                ptr::null(),
                GFALSE,
                G_PARAM_READWRITE | E_SOURCE_PARAM_SETTING,
            )
        }
    }

    unsafe fn from_gvalue(value: *const GValue) -> bool {
        // SAFETY: a boolean `GValue`, by the contract of the caller.
        unsafe { g_value_get_boolean(value) != GFALSE }
    }

    unsafe fn to_gvalue(self, value: *mut GValue) {
        // SAFETY: a boolean `GValue`, by the contract of the caller.
        unsafe { g_value_set_boolean(value, self as _) };
    }
}

impl SettingValue for i64 {
    type Cell = AtomicI64;
    const GTYPE: GType = G_TYPE_INT64;

    fn new_cell() -> AtomicI64 {
        AtomicI64::new(0)
    }

    fn load(cell: &AtomicI64) -> i64 {
        cell.load(Ordering::Relaxed)
    }

    fn store(cell: &AtomicI64, value: i64) {
        cell.store(value, Ordering::Relaxed);
    }

    unsafe fn param_spec(name: &CStr) -> *mut GParamSpec {
        // SAFETY: `name` lives for the process, by the contract of the caller.
        unsafe {
            g_param_spec_int64(
                name.as_ptr(),
                ptr::null(),
                ptr::null(),
                i64::MIN,
                i64::MAX,
                0,
                G_PARAM_READWRITE | E_SOURCE_PARAM_SETTING,
            )
        }
    }

    unsafe fn from_gvalue(value: *const GValue) -> i64 {
        // SAFETY: an int64 `GValue`, by the contract of the caller.
        unsafe { g_value_get_int64(value) }
    }

    unsafe fn to_gvalue(self, value: *mut GValue) {
        // SAFETY: an int64 `GValue`, by the contract of the caller.
        unsafe { g_value_set_int64(value, self) };
    }
}

const PROP_VALUE: u32 = 1;

/// The instance struct: the state of [`ESourceExtension`] and the value.
#[repr(C)]
pub struct SettingExtension<S: Setting> {
    parent: ESourceExtension,
    value: <S::Value as SettingValue>::Cell,
    _setting: PhantomData<fn() -> S>,
}

/// The class struct: only the state of [`ESourceExtensionClass`].
#[repr(C)]
pub struct SettingExtensionClass<S: Setting> {
    parent_class: ESourceExtensionClass,
    _setting: PhantomData<fn() -> S>,
}

// SAFETY: both structs are #[repr(C)] and start with the instance and class
// structs of ESourceExtension. The `PhantomData` fields have no size.
// tests/layout.rs of eds-sys checks the parent layout against `g_type_query`.
// ESourceExtension derives from GObject.
unsafe impl<S: Setting> ObjectSubclass for SettingExtension<S> {
    const NAME: &'static CStr = S::TYPE_NAME;
    type Instance = Self;
    type Class = SettingExtensionClass<S>;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_source_extension_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` starts with `ESourceExtensionClass`. This is the
        // field that `source_find_extension_classes_rec` reads to match an
        // extension name to a `GType`.
        unsafe { (*class).parent_class.name = S::EXTENSION_NAME.as_ptr() };

        // SAFETY: the class struct starts with `ESourceExtensionClass`, which
        // starts with `GObjectClass`. `S::PROPERTY` is `'static`.
        let object_class = class.cast::<GObjectClass>();
        unsafe {
            (*object_class).set_property = Some(set_property::<S>);
            (*object_class).get_property = Some(get_property::<S>);
            g_object_class_install_property(
                object_class,
                PROP_VALUE,
                S::Value::param_spec(S::PROPERTY),
            );
        }
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // SAFETY: a new zeroed instance. The code writes the default and does
        // not assume that zero is its bit pattern.
        unsafe { ptr::addr_of_mut!((*instance).value).write(S::Value::new_cell()) };
    }
}

/// Writes the property. The EDS `.source` parser calls this through
/// `g_object_set` when it restores an account. A panic that unwinds out of it
/// is undefined behaviour, so it has a guard.
unsafe extern "C" fn set_property<S: Setting>(
    object: *mut GObject,
    id: u32,
    value: *mut GValue,
    _pspec: *mut GParamSpec,
) {
    let name = S::TYPE_NAME.to_string_lossy();
    guard(&format!("{name}::set_property"), (), || {
        if id != PROP_VALUE {
            log_critical(&format!(
                "{name} has no property with id {id}; the value is dropped"
            ));
            return;
        }
        // SAFETY: GObject passes an instance of this type, after
        // `instance_init`. `value` holds the type that the pspec declares.
        let extension = unsafe { &*object.cast::<SettingExtension<S>>() };
        S::Value::store(&extension.value, unsafe { S::Value::from_gvalue(value) });
    });
}

/// Reads the property. This is the opposite of [`set_property`].
unsafe extern "C" fn get_property<S: Setting>(
    object: *mut GObject,
    id: u32,
    value: *mut GValue,
    _pspec: *mut GParamSpec,
) {
    let name = S::TYPE_NAME.to_string_lossy();
    guard(&format!("{name}::get_property"), (), || {
        if id != PROP_VALUE {
            log_critical(&format!(
                "{name} has no property with id {id}; nothing is read"
            ));
            return;
        }
        // SAFETY: as in `set_property`. `value` is the `GValue` to fill.
        let extension = unsafe { &*object.cast::<SettingExtension<S>>() };
        unsafe { S::Value::load(&extension.value).to_gvalue(value) };
    });
}

/// Registers the extension of `S`, or finds the existing registration. After
/// this, `e_source_get_extension` matches `S::EXTENSION_NAME` to it.
pub fn ensure_registered<S: Setting>() {
    register_static::<SettingExtension<S>>();
}

/// Returns the value of `S` in `source`, or `None` if `source` has no group
/// for it. A read does not add the extension to the source.
///
/// # Safety
///
/// `source` must be a valid `ESource` that stays alive for the call.
pub unsafe fn read<S: Setting>(source: *mut ESource) -> Option<S::Value> {
    ensure_registered::<S>();
    // SAFETY: the contract above, and the name belongs to this type.
    let extension =
        unsafe { extension_if_present::<SettingExtension<S>>(source, S::EXTENSION_NAME) }?;
    // SAFETY: a live extension of this type, owned by the source.
    Some(S::Value::load(&unsafe { &*extension }.value))
}

/// Writes `value` to `source`, and adds the extension if it is not there.
///
/// The write goes through the GObject property, so it emits `notify`. The
/// registry sends the changed source to Evolution on that signal.
///
/// # Safety
///
/// `source` must be a valid `ESource` that stays alive for the call.
pub unsafe fn write<S: Setting>(source: *mut ESource, value: S::Value) {
    ensure_registered::<S>();
    // SAFETY: the contract above, and the type is registered. The source owns
    // the extension.
    let extension = unsafe { e_source_get_extension(source, S::EXTENSION_NAME.as_ptr()) };
    // SAFETY: a live extension of this type, with the property of `S`. The
    // GValue is initialised before the write and unset after it.
    unsafe {
        let mut gvalue: GValue = std::mem::zeroed();
        g_value_init(&mut gvalue, S::Value::GTYPE);
        value.to_gvalue(&mut gvalue);
        g_object_set_property(extension.cast(), S::PROPERTY.as_ptr(), &gvalue);
        g_value_unset(&mut gvalue);
    }
}
