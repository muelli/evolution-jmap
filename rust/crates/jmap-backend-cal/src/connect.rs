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
use jmap_backend_core::connect::{Collection, ConnectError, connect_with, resolve};
use jmap_backend_core::source::{self, SourceConfig};
use jmap_cal_sync::CalSync;
use jmap_client::Credentials;
use jmap_proto::session::CAPABILITY_CALENDARS;

pub use jmap_backend_core::connect::{ACCEPTED_AUTH_RESULT, write_auth_result};

/// Connects to the server `config` names and resolves which JMAP calendar the
/// source stands for.
///
/// `credentials` are already resolved: whether this account authenticates with
/// a password out of libsecret or with an OAuth 2.0 bearer token is
/// [`jmap_backend_core::connect::connect_with`]'s decision, taken once so that
/// a calendar and an address book on one account cannot disagree about it.
///
/// The client is built with no cancellation of its own: what stops the connect
/// is the scope `connect_sync` installed, and what stops every operation after
/// it is the scope that operation's vfunc installs. See
/// [`jmap_backend_core::connect::connect_with`] for why a flag on the client
/// would be the wrong lifetime.
pub fn open_calendar(
    config: &SourceConfig,
    credentials: Credentials,
) -> Result<CalSync, ConnectError> {
    let client = source::connect(&config.target, credentials)?;
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
