// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Address book connection resolution for `connect_sync`.

use eds_sys::{ENamedParameters, ESource, ESourceAuthenticationResult};
use gio_sys::GCancellable;
use glib_sys::GError;
use jmap_backend_core::connect::{
    Collection, CollectionCandidate, ConnectError, OpenCollection, connect_with, resolve_collection,
};
use jmap_backend_core::source::{self, SourceConfig};
use jmap_book_sync::BookSync;
use jmap_client::Credentials;
use jmap_proto::session::CAPABILITY_CONTACTS;

pub use jmap_backend_core::connect::{ACCEPTED_AUTH_RESULT, write_auth_result};

/// An opened address book connection paired with whether it is writable in EDS.
pub type OpenBook = OpenCollection<BookSync>;

/// Connects to the configured JMAP account and resolves the target address book.
pub fn open_book(
    config: &SourceConfig,
    credentials: Credentials,
) -> Result<OpenBook, ConnectError> {
    open_book_with_account(config, credentials, None)
}

/// Connects to the configured JMAP account and resolves the target address book.
///
/// The account is `account_id` when given, else the source resource identity, else the primary account.
pub fn open_book_with_account(
    config: &SourceConfig,
    credentials: Credentials,
    account_id: Option<&str>,
) -> Result<OpenBook, ConnectError> {
    let client = source::connect(&config.target, config.rebase_urls, credentials)?;
    let open = resolve_collection(
        client,
        Collection::AddressBook,
        CAPABILITY_CONTACTS,
        account_id.or(config.account_id()),
        config.collection_id(),
        |client, account_id| {
            Ok(client
                .address_books(account_id)?
                .into_iter()
                .map(|book| CollectionCandidate {
                    id: book.id,
                    is_default: book.is_default,
                    is_writable: book.my_rights.as_ref().is_none_or(|r| r.is_writable()),
                })
                .collect())
        },
        BookSync::new,
    )?;

    tracing::debug!(
        account_id = open.sync.account_id().as_str(),
        address_book_id = open.sync.address_book_id().as_str(),
        writable = open.writable,
        "opened JMAP address book"
    );

    Ok(open)
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
    // SAFETY: Arguments satisfy the connect_with contract.
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
