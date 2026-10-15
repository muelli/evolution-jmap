// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendar connection resolution for `connect_sync`.

use eds_sys::{ENamedParameters, ESource, ESourceAuthenticationResult};
use gio_sys::GCancellable;
use glib_sys::GError;
use jmap_backend_core::connect::{Collection, ConnectError, connect_with, resolve};
use jmap_backend_core::source::{self, SourceConfig};
use jmap_cal_sync::CalSync;
use jmap_client::Credentials;
use jmap_proto::session::CAPABILITY_CALENDARS;

pub use jmap_backend_core::connect::{ACCEPTED_AUTH_RESULT, write_auth_result};

/// Connects to the configured JMAP account and resolves the target calendar.
pub fn open_calendar(
    config: &SourceConfig,
    credentials: Credentials,
) -> Result<CalSync, ConnectError> {
    let client = source::connect(&config.target, config.rebase_urls, credentials)?;
    let account_id = client.primary_account(CAPABILITY_CALENDARS)?;
    let calendars = client.calendars(&account_id)?;

    let calendar_id = resolve(
        Collection::Calendar,
        config.resource_id.as_deref(),
        calendars
            .iter()
            .map(|calendar| (calendar.id.as_ref(), calendar.is_default)),
    )?;

    tracing::debug!(
        account_id = account_id.as_str(),
        calendar_id = calendar_id.as_str(),
        "opened JMAP calendar"
    );

    Ok(CalSync::new(client, account_id, calendar_id))
}

/// Connects to the JMAP server for `source` and returns an active [`CalSync`].
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
) -> Option<CalSync> {
    // SAFETY: the arguments satisfy `connect_with`'s contract by this
    // function's, which is the same one.
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
