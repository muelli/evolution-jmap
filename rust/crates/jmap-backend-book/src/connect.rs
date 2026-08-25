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
use jmap_backend_core::connect::{Collection, ConnectError, connect_with, resolve};
use jmap_backend_core::source::{self, SourceConfig};
use jmap_book_sync::BookSync;
use jmap_client::Credentials;
use jmap_proto::session::CAPABILITY_CONTACTS;

pub use jmap_backend_core::connect::{ACCEPTED_AUTH_RESULT, write_auth_result};

/// Connects to the server `config` names and resolves which JMAP address book
/// the source stands for.
///
/// `credentials` are already resolved: whether this account authenticates with
/// a password out of libsecret or with an OAuth 2.0 bearer token is
/// [`jmap_backend_core::connect::connect_with`]'s decision, taken once so that
/// an address book and a calendar on one account cannot disagree about it.
///
/// The client is built with no cancellation of its own: what stops the connect
/// is the scope `connect_sync` installed, and what stops every operation after
/// it is the scope that operation's vfunc installs. See
/// [`jmap_backend_core::connect::connect_with`] for why a flag on the client
/// would be the wrong lifetime.
pub fn open_book(
    config: &SourceConfig,
    credentials: Credentials,
) -> Result<BookSync, ConnectError> {
    let client = source::connect(&config.target, credentials)?;
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
