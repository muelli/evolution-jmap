// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Opening the calendar `connect_sync` needs.
//!
//! The mirror of `jmap-backend-book`'s module of the same name, and just as
//! short for the same reason: everything that is not about *events* lives in
//! [`jmap_backend_core::connect`]. What is left is the session capability the
//! account is looked up under — `urn:ietf:params:jmap:calendars`, not the
//! contacts one — and the list the source's `[Resource] Identity` is resolved
//! against.
//!
//! That identity field is the one place the two backends read the same key and
//! mean different things: `Identity=Cal1` names a calendar here and an address
//! book there. It is why `SourceConfig` calls it `resource_id` rather than
//! either, and why resolving it is the caller's job and not the config's.

use eds_sys::{ENamedParameters, ESource, ESourceAuthenticationResult};
use gio_sys::GCancellable;
use glib_sys::GError;
use jmap_backend_core::connect::{Collection, ConnectError, connect_with, credentials, resolve};
use jmap_backend_core::source::SourceConfig;
use jmap_cal_sync::CalSync;
use jmap_client::Client;
use jmap_client::transport::CancelFlag;
use jmap_proto::session::CAPABILITY_CALENDARS;

pub use jmap_backend_core::connect::{ACCEPTED_AUTH_RESULT, write_auth_result};

/// Connects to the server `config` names and resolves which JMAP calendar the
/// source stands for.
///
/// `password` is what EDS got out of libsecret, which is `None` on the first
/// attempt; see [`jmap_backend_core::connect::credentials`] for what that
/// means for the prompt.
pub fn open_calendar(
    config: &SourceConfig,
    password: Option<&str>,
    cancel: CancelFlag,
) -> Result<CalSync, ConnectError> {
    let credentials = credentials(config.user.as_deref(), password)?;

    let client = Client::builder()
        .cancel_flag(cancel)
        .connect(&config.origin, credentials)?;
    let account_id = client.primary_account(CAPABILITY_CALENDARS)?;
    let calendars = client.calendars(&account_id)?;

    let calendar_id = resolve(
        Collection::Calendar,
        config.resource_id.as_deref(),
        calendars
            .iter()
            .map(|calendar| (calendar.id.as_ref(), calendar.is_default)),
    )?;

    Ok(CalSync::new(client, account_id, calendar_id))
}

/// The whole of `connect_sync` except the instance: from the `ESource` EDS
/// hands the backend to a [`CalSync`], with `out_auth_result` and `error`
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
