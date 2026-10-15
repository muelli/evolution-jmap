// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Address book connection resolution for `connect_sync`.

use eds_sys::{ENamedParameters, ESource, ESourceAuthenticationResult};
use gio_sys::GCancellable;
use glib_sys::GError;
use jmap_backend_core::connect::{Collection, ConnectError, connect_with, resolve};
use jmap_backend_core::source::{self, SourceConfig};
use jmap_book_sync::BookSync;
use jmap_client::Credentials;
use jmap_proto::session::CAPABILITY_CONTACTS;

pub use jmap_backend_core::connect::{ACCEPTED_AUTH_RESULT, write_auth_result};

/// Connects to the configured JMAP account and resolves the target address book.
pub fn open_book(
    config: &SourceConfig,
    credentials: Credentials,
) -> Result<BookSync, ConnectError> {
    let client = source::connect(&config.target, config.rebase_urls, credentials)?;
    let account_id = client.primary_account(CAPABILITY_CONTACTS)?;
    let books = client.address_books(&account_id)?;

    let address_book_id = resolve(
        Collection::AddressBook,
        config.resource_id.as_deref(),
        books.iter().map(|book| (book.id.as_ref(), book.is_default)),
    )?;

    tracing::debug!(
        account_id = account_id.as_str(),
        address_book_id = address_book_id.as_str(),
        "opened JMAP address book"
    );

    Ok(BookSync::new(client, account_id, address_book_id))
}

/// Connects to the JMAP server for `source` and returns an active [`BookSync`].
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
) -> Option<BookSync> {
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
