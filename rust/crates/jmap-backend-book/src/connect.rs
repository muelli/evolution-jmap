// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opening the address book `connect_sync` needs.
//!
//! All that is left here is the part that is about *contacts*: which session
//! capability the account is looked up under, and which list the source's
//! `[Resource] Identity` is resolved against. The rest — the credentials, the
//! `out_auth_result` classification EDS re-prompts on, the `ESource` and the
//! two out-parameters — is [`jmap_backend_core::connect`], because the
//! calendar backend answers those questions the same way and a rule about
//! password prompts that exists twice is a rule that gets corrected once.

use eds_sys::{ENamedParameters, ESource, ESourceAuthenticationResult};
use gio_sys::GCancellable;
use glib_sys::GError;
use jmap_backend_core::connect::{Collection, ConnectError, connect_with, credentials, resolve};
use jmap_backend_core::source::SourceConfig;
use jmap_book_sync::BookSync;
use jmap_client::Client;
use jmap_client::transport::CancelFlag;
use jmap_proto::session::CAPABILITY_CONTACTS;

pub use jmap_backend_core::connect::{ACCEPTED_AUTH_RESULT, write_auth_result};

/// Connects to the server `config` names and resolves which JMAP address book
/// the source stands for.
///
/// `password` is what EDS got out of libsecret, which is `None` on the first
/// attempt; see [`jmap_backend_core::connect::credentials`] for what that
/// means for the prompt.
pub fn open_book(
    config: &SourceConfig,
    password: Option<&str>,
    cancel: CancelFlag,
) -> Result<BookSync, ConnectError> {
    let credentials = credentials(config.user.as_deref(), password)?;

    let client = Client::builder()
        .cancel_flag(cancel)
        .connect(&config.origin, credentials)?;
    let account_id = client.primary_account(CAPABILITY_CONTACTS)?;
    let books = client.address_books(&account_id)?;

    let address_book_id = resolve(
        Collection::AddressBook,
        config.resource_id.as_deref(),
        books.iter().map(|book| (book.id.as_ref(), book.is_default)),
    )?;

    Ok(BookSync::new(client, account_id, address_book_id))
}

/// The whole of `connect_sync` except the instance: from the `ESource` EDS
/// hands the backend to a [`BookSync`], with `out_auth_result` and `error`
/// written the way the vfunc has to write them.
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
