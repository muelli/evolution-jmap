// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapFiltersExtension`: the `EExtension` on `EMailConfigNotebook` that
//! adds the Server Filters page to the account editor, for JMAP accounts
//! only.
//!
//! It is the vacation extension with another page. Read
//! [`crate::vacation::extension`] for why `constructed` does the work and
//! why the gate here is the backend name of the account source. The
//! asynchronous load of the page finds out if the *server* offers Sieve
//! ([`crate::filters::page`]).

use std::ffi::CStr;

use eds_sys::{
    E_SOURCE_EXTENSION_MAIL_ACCOUNT, EExtension, ESourceBackend, e_extension_get_extensible,
    e_source_backend_get_backend_name,
};
use evo_sys::{
    EMailConfigNotebook, e_mail_config_notebook_add_page,
    e_mail_config_notebook_get_account_source, e_mail_config_notebook_get_collection_source,
    e_mail_config_notebook_get_type,
};
use glib_sys::GType;
use jmap_backend_core::BACKEND_NAME;
use jmap_backend_core::extension::{Extension, ExtensionObject};
use jmap_backend_core::marshal::{extension_if_present, read_string};

use crate::filters::page;

/// The Server Filters page of the account editor.
pub struct Filters;

/// The registered `JmapFiltersExtension` type.
pub type JmapFiltersExtension = ExtensionObject<Filters>;

impl Extension for Filters {
    const TYPE_NAME: &'static CStr = c"JmapFiltersExtension";
    const EXTENSIBLE_NAME: &'static CStr = c"EMailConfigNotebook";

    fn extensible_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_mail_config_notebook_get_type() }
    }

    unsafe fn constructed(extension: *mut EExtension) {
        // SAFETY: the contract of this method.
        unsafe { constructed(extension) }
    }
}

/// Checks the account and adds the page.
unsafe fn constructed(extension: *mut EExtension) {
    unsafe {
        // SAFETY: `extension` is live. The extensible is the notebook of this
        // extension, and it stays alive during this call. The cast is the
        // same as E_MAIL_CONFIG_NOTEBOOK() in C.
        let notebook: *mut EMailConfigNotebook = e_extension_get_extensible(extension).cast();
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

        tracing::trace!("adding the server filters page to a JMAP account's editor");
        // SAFETY: the source is valid, because the notebook owns it.
        let page = page::create(connect_source);
        if !page.is_null() {
            e_mail_config_notebook_add_page(notebook, page);
        }
    }
}
