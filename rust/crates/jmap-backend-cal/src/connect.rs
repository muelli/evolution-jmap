// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendar connection resolution for `connect_sync`.

use eds_sys::{ENamedParameters, ESource, ESourceAuthenticationResult};
use gio_sys::GCancellable;
use glib_sys::GError;
use jmap_backend_core::connect::{
    Collection, CollectionCandidate, ConnectError, OpenCollection, connect_with, resolve_collection,
};
use jmap_backend_core::source::{self, SourceConfig};
use jmap_cal_sync::CalSync;
use jmap_client::Credentials;
use jmap_proto::session::CAPABILITY_CALENDARS;

pub use jmap_backend_core::connect::{ACCEPTED_AUTH_RESULT, write_auth_result};

/// An opened calendar connection paired with whether it is writable in EDS.
pub type OpenCalendar = OpenCollection<CalSync>;

/// Connects to the configured JMAP account and resolves the target calendar.
pub fn open_calendar(
    config: &SourceConfig,
    credentials: Credentials,
) -> Result<OpenCalendar, ConnectError> {
    open_calendar_with_account(config, credentials, None)
}

/// Connects to the configured JMAP account and resolves the target calendar.
///
/// Resolves `account_id` when provided. Falls back to identity configuration
/// or the primary calendar account.
pub fn open_calendar_with_account(
    config: &SourceConfig,
    credentials: Credentials,
    account_id: Option<&str>,
) -> Result<OpenCalendar, ConnectError> {
    let client = source::connect(&config.target, config.rebase_urls, credentials)?;
    let open = resolve_collection(
        client,
        Collection::Calendar,
        CAPABILITY_CALENDARS,
        account_id.or(config.account_id()),
        config.collection_id(),
        |client, account_id| {
            Ok(client
                .calendars(account_id)?
                .into_iter()
                .map(|calendar| CollectionCandidate {
                    id: calendar.id,
                    is_default: calendar.is_default,
                    is_writable: calendar.my_rights.as_ref().is_none_or(|r| r.is_writable()),
                })
                .collect())
        },
        CalSync::new,
    )?;

    tracing::debug!(
        account_id = open.sync.account_id().as_str(),
        calendar_id = open.sync.calendar_id().as_str(),
        writable = open.writable,
        "opened JMAP calendar"
    );

    Ok(open)
}

/// Connects to the JMAP server for `source` and returns an active [`OpenCalendar`].
///
/// # Safety
///
/// Follows safety requirements of [`jmap_backend_core::connect::connect_with`].
pub unsafe fn connect(
    source: *mut ESource,
    credentials: *const ENamedParameters,
    cancellable: *mut GCancellable,
    out_auth_result: *mut ESourceAuthenticationResult,
    error: *mut *mut GError,
) -> Option<OpenCalendar> {
    // SAFETY: Arguments satisfy `connect_with` safety invariants.
    unsafe {
        connect_with(
            Collection::Calendar,
            source,
            credentials,
            cancellable,
            out_auth_result,
            error,
            open_calendar,
        )
    }
}
