// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendar connection resolution for `connect_sync`.

use std::fmt;
use std::ops::{Deref, DerefMut};

use eds_sys::{ENamedParameters, ESource, ESourceAuthenticationResult};
use gio_sys::GCancellable;
use glib_sys::GError;
use jmap_backend_core::connect::{Collection, ConnectError, connect_with, resolve};
use jmap_backend_core::source::{self, SourceConfig};
use jmap_cal_sync::CalSync;
use jmap_client::Credentials;
use jmap_proto::Id;
use jmap_proto::session::CAPABILITY_CALENDARS;

pub use jmap_backend_core::connect::{ACCEPTED_AUTH_RESULT, write_auth_result};

/// An opened calendar connection paired with whether it is writable in EDS.
pub struct OpenCalendar {
    pub sync: CalSync,
    pub writable: bool,
}

impl fmt::Debug for OpenCalendar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenCalendar")
            .field("account_id", self.sync.account_id())
            .field("calendar_id", self.sync.calendar_id())
            .field("writable", &self.writable)
            .finish()
    }
}

impl Deref for OpenCalendar {
    type Target = CalSync;

    fn deref(&self) -> &Self::Target {
        &self.sync
    }
}

impl DerefMut for OpenCalendar {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.sync
    }
}

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
    let account_id = match account_id.or(config.account_id()) {
        Some(id) => Id::from(id),
        None => client.primary_account(CAPABILITY_CALENDARS)?,
    };
    let calendars = client.calendars(&account_id)?;

    let calendar_id = resolve(
        Collection::Calendar,
        config.collection_id(),
        calendars
            .iter()
            .map(|calendar| (calendar.id.as_ref(), calendar.is_default)),
    )?;

    let calendar = calendars
        .iter()
        .find(|c| c.id.as_ref() == Some(&calendar_id));
    let is_account_read_only = client
        .session()
        .accounts
        .get(&account_id)
        .is_some_and(|a| a.is_read_only);

    let writable = !is_account_read_only
        && calendar
            .and_then(|c| c.my_rights.as_ref())
            .is_none_or(|r| r.is_writable());

    tracing::debug!(
        account_id = account_id.as_str(),
        calendar_id = calendar_id.as_str(),
        writable,
        "opened JMAP calendar"
    );

    Ok(OpenCalendar {
        sync: CalSync::new(client, account_id, calendar_id),
        writable,
    })
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
