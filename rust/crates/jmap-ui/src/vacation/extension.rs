// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapVacationExtension`: the `EExtension` on `EMailConfigNotebook` that
//! adds the vacation page to the account editor. It adds the page only for
//! JMAP accounts.
//!
//! The structure is the same as `JmapConfigLookup` (jmap-config), which
//! copies `e-webdav-config-lookup.c`. `class_init` writes `extensible_type`.
//! `constructed` does the work, because the extensible object is available
//! there.
//!
//! The gate is level 1 only (the backend name of the account source). This
//! is the only check that a dialog callback can do synchronously. The
//! asynchronous load of the page finds out if the *server* offers the
//! capability ([`crate::vacation::page`]).

use std::ffi::CStr;

use eds_sys::{
    E_SOURCE_EXTENSION_MAIL_ACCOUNT, EExtension, EExtensionClass, ESourceBackend,
    e_extension_get_extensible, e_extension_get_type, e_source_backend_get_backend_name,
};
use evo_sys::{
    EMailConfigNotebook, e_mail_config_notebook_add_page,
    e_mail_config_notebook_get_account_source, e_mail_config_notebook_get_collection_source,
    e_mail_config_notebook_get_type,
};
use glib_sys::GType;
use gobject_sys::{GObject, GObjectClass};
use jmap_backend_core::marshal::{extension_if_present, read_string};
use jmap_backend_core::subclass::{self, ObjectSubclass};
use jmap_backend_core::trampoline::guard;

use crate::vacation::page;

/// The only backend name that this module accepts. It is the protocol of the
/// Camel provider and also the factory name of the collection backend.
/// `account::BACKEND_NAME` in jmap-config has the same value. This crate does
/// not import it, because a dependency for one literal costs more than the
/// literal.
const BACKEND_NAME: &CStr = c"jmap";

/// The instance: only the state of `EExtension`.
#[repr(C)]
pub struct JmapVacationExtension {
    parent: EExtension,
}

/// The class: only the state of `EExtensionClass`.
#[repr(C)]
pub struct JmapVacationExtensionClass {
    parent_class: EExtensionClass,
}

// SAFETY: both structs are #[repr(C)] and start with the EExtension structs.
// EExtension derives from GObject. The eds-sys layout tests compare its size
// with g_type_query.
unsafe impl ObjectSubclass for JmapVacationExtension {
    const NAME: &'static CStr = c"JmapVacationExtension";
    type Instance = JmapVacationExtension;
    type Class = JmapVacationExtensionClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initializes itself.
        unsafe { e_extension_get_type() }
    }

    fn class_init_types() -> Vec<GType> {
        // SAFETY: no arguments, and the type initializes itself. This call
        // registers the type before `class_init`, which can only look it up.
        vec![unsafe { e_mail_config_notebook_get_type() }]
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` starts with `EExtensionClass`.
        // `e_extensible_load_extensions` compares its `extensible_type`.
        unsafe {
            (*class).parent_class.extensible_type =
                subclass::registered_type(c"EMailConfigNotebook")
        };

        // SAFETY: the class starts with GObjectClass indirectly, as in JmapConfigLookup.
        let object_class = class.cast::<GObjectClass>();
        unsafe { (*object_class).constructed = Some(constructed) };
    }
}

/// Chains up, then checks the account and adds the page. When the notebook
/// constructs its extensions, it already knows its sources.
unsafe extern "C" fn constructed(object: *mut GObject) {
    guard("JmapVacationExtension::constructed", (), || unsafe {
        // SAFETY: the parent class of a live instance is initialized and
        // alive. The vfunc contract requires the chain-up.
        let parent = subclass::parent_class::<GObjectClass>(JmapVacationExtension::parent_type());
        if let Some(chained) = parent.and_then(|class| class.constructed) {
            chained(object);
        }

        // SAFETY: GObject passes a live instance of this type. The extensible
        // is the notebook of this extension, and it stays alive during this
        // call. The cast is the same as E_MAIL_CONFIG_NOTEBOOK() in C.
        let notebook: *mut EMailConfigNotebook =
            e_extension_get_extensible(object.cast::<EExtension>()).cast();
        let account_source = e_mail_config_notebook_get_account_source(notebook);
        if account_source.is_null() {
            return;
        }

        // Level 1: is this a JMAP account? The editor writes the provider
        // protocol as `[Mail Account] BackendName=jmap`.
        // SAFETY: the notebook owns this live source. The source owns the
        // extension and its string.
        let backend_name =
            extension_if_present::<ESourceBackend>(account_source, E_SOURCE_EXTENSION_MAIL_ACCOUNT)
                .and_then(|backend| read_string(e_source_backend_get_backend_name(backend)));
        if backend_name.as_deref() != BACKEND_NAME.to_str().ok() {
            return;
        }

        // A JMAP account keeps [Authentication] and [Security] on the
        // collection source. A mail account without a collection keeps them
        // on the account source.
        let collection_source = e_mail_config_notebook_get_collection_source(notebook);
        let connect_source = if collection_source.is_null() {
            account_source
        } else {
            collection_source
        };

        tracing::trace!("adding the vacation page to a JMAP account's editor");
        // SAFETY: the source is valid, because the notebook owns it.
        let page = page::create(connect_source);
        if !page.is_null() {
            e_mail_config_notebook_add_page(notebook, page);
        }
    });
}
