// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapVacationExtension`: the `EExtension` on `EMailConfigNotebook` that
//! puts the vacation page into the account editor — for JMAP accounts, and
//! for nobody else.
//!
//! The shape is `JmapConfigLookup`'s (jmap-config), which is
//! `e-webdav-config-lookup.c`'s: `extensible_type` written in `class_init`,
//! the actual work in `constructed`, where the extensible this instance was
//! made for is in hand. The gate is level 1 only — the account source's
//! backend name — because it is the one question answerable synchronously in
//! a dialog callback; whether the *server* offers the capability is the
//! page's own async load to find out ([`crate::vacation::page`]).

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

/// The one backend name this module dresses: the Camel provider's protocol
/// and the collection backend's factory name alike. jmap-config's
/// `account::BACKEND_NAME` is the same constant; not imported, because a
/// dependency edge for one literal would be heavier than the literal.
const BACKEND_NAME: &CStr = c"jmap";

/// The instance: `EExtension`'s own state and nothing else.
#[repr(C)]
pub struct JmapVacationExtension {
    parent: EExtension,
}

/// The class: `EExtensionClass`'s own state and nothing else.
#[repr(C)]
pub struct JmapVacationExtensionClass {
    parent_class: EExtensionClass,
}

// SAFETY: both structs are #[repr(C)] and lead with EExtension's own structs;
// EExtension derives from GObject (eds-sys's layout tests hold its size
// against g_type_query).
unsafe impl ObjectSubclass for JmapVacationExtension {
    const NAME: &'static CStr = c"JmapVacationExtension";
    type Instance = JmapVacationExtension;
    type Class = JmapVacationExtensionClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_extension_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` leads with `EExtensionClass`, whose
        // `extensible_type` is what `e_extensible_load_extensions` matches.
        unsafe { (*class).parent_class.extensible_type = e_mail_config_notebook_get_type() };

        // SAFETY: transitively leads with GObjectClass, as in JmapConfigLookup.
        let object_class = class.cast::<GObjectClass>();
        unsafe { (*object_class).constructed = Some(constructed) };
    }
}

/// Chains up, then gates and adds the page: by the time the notebook
/// constructs its extensions it already knows its sources, so this is the
/// place both questions can be asked.
unsafe extern "C" fn constructed(object: *mut GObject) {
    guard("JmapVacationExtension::constructed", (), || unsafe {
        // SAFETY: the parent class of a live instance is initialised and
        // alive; chaining up is the vfunc's contract.
        let parent = subclass::parent_class::<GObjectClass>(JmapVacationExtension::parent_type());
        if let Some(chained) = parent.and_then(|class| class.constructed) {
            chained(object);
        }

        // SAFETY: GObject passes a live instance of this type; the extensible
        // is the notebook this extension was instantiated for, alive across
        // this call. The cast is C's E_MAIL_CONFIG_NOTEBOOK().
        let notebook: *mut EMailConfigNotebook =
            e_extension_get_extensible(object.cast::<EExtension>()).cast();
        let account_source = e_mail_config_notebook_get_account_source(notebook);
        if account_source.is_null() {
            return;
        }

        // Level 1: is this account ours at all? `[Mail Account]
        // BackendName=jmap` is the editor-side spelling of the provider
        // protocol.
        // SAFETY: a live source owned by the notebook; the extension and its
        // string are the source's own.
        let backend_name =
            extension_if_present::<ESourceBackend>(account_source, E_SOURCE_EXTENSION_MAIL_ACCOUNT)
                .and_then(|backend| read_string(e_source_backend_get_backend_name(backend)));
        if backend_name.as_deref() != BACKEND_NAME.to_str().ok() {
            return;
        }

        // The collection source is where a JMAP account's [Authentication]
        // and [Security] live; a bare mail account (no collection) keeps them
        // on the account source itself.
        let collection_source = e_mail_config_notebook_get_collection_source(notebook);
        let connect_source = if collection_source.is_null() {
            account_source
        } else {
            collection_source
        };

        tracing::trace!("adding the vacation page to a JMAP account's editor");
        // SAFETY: a valid source, per the notebook's own ownership.
        let page = page::create(connect_source);
        if !page.is_null() {
            e_mail_config_notebook_add_page(notebook, page);
        }
    });
}
