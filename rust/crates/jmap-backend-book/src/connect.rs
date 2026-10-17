// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Address book connection resolution for `connect_sync`.

use std::fmt;
use std::ops::{Deref, DerefMut};

use eds_sys::{ENamedParameters, ESource, ESourceAuthenticationResult};
use gio_sys::GCancellable;
use glib_sys::GError;
use jmap_backend_core::connect::{Collection, ConnectError, connect_with, resolve};
use jmap_backend_core::source::{self, SourceConfig};
use jmap_book_sync::BookSync;
use jmap_client::Credentials;
use jmap_proto::session::CAPABILITY_CONTACTS;

pub use jmap_backend_core::connect::{ACCEPTED_AUTH_RESULT, write_auth_result};

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
    let client = source::connect(&config.target, config.rebase_urls, credentials)?;
    let account_id = client.primary_account(CAPABILITY_CONTACTS)?;
    let books = client.address_books(&account_id)?;

    let address_book_id = resolve(
        Collection::AddressBook,
        config.resource_id.as_deref(),
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
        connect_with(
            Collection::AddressBook,
            source,
            credentials,
            cancellable,
            out_auth_result,
            error,
            open_book,
        )
    }
}
