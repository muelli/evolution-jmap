// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! An `EExtension` subclass that does its work in `constructed`, as
//! `e-webdav-config-lookup.c` does.
//!
//! Each [`Extension`] gives one registered type, [`ExtensionObject`] of that
//! extension. The class names the extensible type, and `constructed` chains
//! up, then calls [`Extension::constructed`] inside a panic guard.

use std::ffi::CStr;
use std::marker::PhantomData;

use eds_sys::{EExtension, EExtensionClass, e_extension_get_type};
use glib_sys::GType;
use gobject_sys::{GObject, GObjectClass};

use crate::subclass::{self, InterfaceDecl, ObjectSubclass};
use crate::trampoline::guard;

/// One extension of an `EExtensible` type.
pub trait Extension: 'static {
    /// The `GType` name of the extension class.
    const TYPE_NAME: &'static CStr;
    /// The `GType` name of the extensible type. `class_init` looks it up by
    /// this name, because it must not call the type accessor.
    const EXTENSIBLE_NAME: &'static CStr;

    /// The accessor of the extensible type. Registration calls it before
    /// `class_init`.
    fn extensible_type() -> GType;

    /// The interfaces that the extension implements.
    fn interfaces() -> Vec<InterfaceDecl> {
        Vec::new()
    }

    /// Does the work of the extension, after the chain-up to the parent.
    ///
    /// # Safety
    ///
    /// `extension` is a live instance of this extension in construction. Its
    /// extensible is set and stays alive during the call.
    unsafe fn constructed(extension: *mut EExtension);
}

/// The instance struct: only the state of [`EExtension`].
#[repr(C)]
pub struct ExtensionObject<E: Extension> {
    parent: EExtension,
    _extension: PhantomData<fn() -> E>,
}

/// The class struct: only the state of [`EExtensionClass`].
#[repr(C)]
pub struct ExtensionObjectClass<E: Extension> {
    parent_class: EExtensionClass,
    _extension: PhantomData<fn() -> E>,
}

// SAFETY: both structs are #[repr(C)] and start with the instance and class
// structs of EExtension. The `PhantomData` fields have no size. EExtension
// derives from GObject. The eds-sys layout tests compare its size with
// `g_type_query`.
unsafe impl<E: Extension> ObjectSubclass for ExtensionObject<E> {
    const NAME: &'static CStr = E::TYPE_NAME;
    type Instance = Self;
    type Class = ExtensionObjectClass<E>;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_extension_get_type() }
    }

    fn interfaces() -> Vec<InterfaceDecl> {
        E::interfaces()
    }

    fn class_init_types() -> Vec<GType> {
        vec![E::extensible_type()]
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` starts with `EExtensionClass`.
        // `e_extensible_load_extensions` compares its `extensible_type`.
        unsafe {
            (*class).parent_class.extensible_type = subclass::registered_type(E::EXTENSIBLE_NAME)
        };

        // SAFETY: the class starts with `EExtensionClass`, which starts with
        // `GObjectClass`.
        let object_class = class.cast::<GObjectClass>();
        unsafe { (*object_class).constructed = Some(constructed::<E>) };
    }
}

/// Chains up, then calls [`Extension::constructed`].
unsafe extern "C" fn constructed<E: Extension>(object: *mut GObject) {
    let context = format!("{}::constructed", E::TYPE_NAME.to_string_lossy());
    guard(&context, (), || unsafe {
        // SAFETY: the parent class of a live instance is initialised and
        // alive. The vfunc contract requires the chain-up.
        let parent = subclass::parent_class::<GObjectClass>(ExtensionObject::<E>::parent_type());
        if let Some(chained) = parent.and_then(|class| class.constructed) {
            chained(object);
        }

        // SAFETY: GObject passes a live instance of this type in construction.
        E::constructed(object.cast());
    });
}
