// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Address book connection resolution for `connect_sync`.

use std::ffi::CStr;
use std::fmt;
use std::ops::{Deref, DerefMut};

use eds_sys::{ENamedParameters, ESource, ESourceAuthenticationResult};
use gio_sys::GCancellable;
use glib_sys::GError;
use jmap_backend_core::connect::{Collection, ConnectError, connect_with, resolve};
use jmap_backend_core::source::{self, SourceConfig};
use jmap_book_sync::BookSync;
use jmap_client::Credentials;
use jmap_proto::Id;
use jmap_proto::session::CAPABILITY_CONTACTS;

pub use jmap_backend_core::connect::{ACCEPTED_AUTH_RESULT, write_auth_result};

const ACCOUNT_ID_DATA_KEY: &CStr = c"AccountId";

/// Sets the JMAP account ID on `source`.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn set_source_account_id(source: *mut ESource, account_id: &str) {
    if source.is_null() {
        return;
    }
    let Ok(cstr) = std::ffi::CString::new(account_id) else {
        return;
    };
    // SAFETY: caller guarantees source is a valid ESource pointer.
    unsafe {
        let copy = glib_sys::g_strdup(cstr.as_ptr());
        gobject_sys::g_object_set_data_full(
            source.cast(),
            ACCOUNT_ID_DATA_KEY.as_ptr(),
            copy.cast(),
            Some(glib_sys::g_free),
        );
    }
}

/// Reads the JMAP account ID from `source`, if configured.
///
/// Returns `None` when absent, signaling that the backend should fall back
/// to the primary account for the capability.
///
/// # Safety
///
/// `source` must be NULL or a valid `ESource`.
pub unsafe fn source_account_id(source: *mut ESource) -> Option<String> {
    if source.is_null() {
        return None;
    }
    // SAFETY: caller guarantees source is a valid ESource pointer.
    unsafe {
        let read_obj = |obj: *mut gobject_sys::GObject, keys: &[&CStr]| -> Option<String> {
            if obj.is_null() {
                return None;
            }
            for key in keys {
                let ptr = gobject_sys::g_object_get_data(obj, key.as_ptr());
                if !ptr.is_null() {
                    let s = CStr::from_ptr(ptr.cast()).to_string_lossy().into_owned();
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
            None
        };

        let keys = [c"account-id", c"AccountId", c"Account", c"jmap-account-id"];
        if let Some(val) = read_obj(source.cast(), &keys) {
            return Some(val);
        }

        eds_sys::e_source_resource_get_type();
        let resource = jmap_backend_core::marshal::extension_if_present::<eds_sys::ESourceResource>(
            source,
            eds_sys::E_SOURCE_EXTENSION_RESOURCE,
        );
        if let Some(resource) = resource
            && let Some(val) = read_obj(resource.cast(), &keys)
        {
            return Some(val);
        }

        if eds_sys::e_source_has_extension(source, c"JMAP Account".as_ptr()) != glib_sys::GFALSE {
            let ext = eds_sys::e_source_get_extension(source, c"JMAP Account".as_ptr());
            if let Some(val) = read_obj(ext.cast(), &keys) {
                return Some(val);
            }
        }

        None
    }
}

/// An opened address book connection paired with whether it is writable in EDS.
pub struct OpenBook {
    pub sync: BookSync,
    pub writable: bool,
}

impl fmt::Debug for OpenBook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenBook")
            .field("account_id", self.sync.account_id())
            .field("address_book_id", self.sync.address_book_id())
            .field("writable", &self.writable)
            .finish()
    }
}

impl Deref for OpenBook {
    type Target = BookSync;

    fn deref(&self) -> &Self::Target {
        &self.sync
    }
}

impl DerefMut for OpenBook {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.sync
    }
}

/// Connects to the configured JMAP account and resolves the target address book.
pub fn open_book(
    config: &SourceConfig,
    credentials: Credentials,
) -> Result<OpenBook, ConnectError> {
    open_book_with_account(config, credentials, None)
}

/// Connects to the configured JMAP account and resolves the target address book.
///
/// The account is `account_id` when given, else the one the source's
/// `[Resource] Identity` names (see [`SourceConfig::account_id`]), else the
/// primary account for the capability.
pub fn open_book_with_account(
    config: &SourceConfig,
    credentials: Credentials,
    account_id: Option<&str>,
) -> Result<OpenBook, ConnectError> {
    let client = source::connect(&config.target, config.rebase_urls, credentials)?;
    let account_id = match account_id.or(config.account_id()) {
        Some(id) => Id::from(id),
        None => client.primary_account(CAPABILITY_CONTACTS)?,
    };
    let books = client.address_books(&account_id)?;

    let address_book_id = resolve(
        Collection::AddressBook,
        config.collection_id(),
        books.iter().map(|book| (book.id.as_ref(), book.is_default)),
    )?;

    let book = books
        .iter()
        .find(|b| b.id.as_ref() == Some(&address_book_id));
    let is_account_read_only = client
        .session()
        .accounts
        .get(&account_id)
        .is_some_and(|a| a.is_read_only);

    let writable = !is_account_read_only
        && book
            .and_then(|b| b.my_rights.as_ref())
            .is_none_or(|r| r.is_writable());

    tracing::debug!(
        account_id = account_id.as_str(),
        address_book_id = address_book_id.as_str(),
        writable,
        "opened JMAP address book"
    );

    Ok(OpenBook {
        sync: BookSync::new(client, account_id, address_book_id),
        writable,
    })
}

/// Connects to the JMAP server for `source` and returns an active [`OpenBook`].
///
/// # Safety
///
/// As [`jmap_backend_core::connect::connect_with`].
pub unsafe fn connect(
    source: *mut ESource,
    credentials: *const ENamedParameters,
    cancellable: *mut GCancellable,
    out_auth_result: *mut ESourceAuthenticationResult,
    error: *mut *mut GError,
) -> Option<OpenBook> {
    // SAFETY: the arguments satisfy `connect_with`'s contract by this
    // function's, which is the same one.
    unsafe {
        let account_id = source_account_id(source);
        connect_with(
            Collection::AddressBook,
            source,
            credentials,
            cancellable,
            out_auth_result,
            error,
            |config, creds| open_book_with_account(config, creds, account_id.as_deref()),
        )
    }
}
