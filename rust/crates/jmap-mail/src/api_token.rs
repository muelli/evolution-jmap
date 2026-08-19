// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether a mail account authenticates with a manually pasted API token —
//! the Camel-side sibling of [`jmap_backend_core::api_token`], read off the
//! same `CamelNetworkSettings:auth-mechanism` field [`crate::oauth2::uses_oauth2`]
//! reads for OAuth 2.0. See that module's docs for why Camel keeps this
//! project's authentication choice on a settings property rather than an
//! `ESource` field, and [`jmap_backend_core::api_token`]'s docs for why the
//! method exists at all.

use eds_sys::{CamelSettings, camel_network_settings_dup_auth_mechanism};

use crate::server::{network, take_string};

/// Whether `settings`' `auth-mechanism` names the API-token method — see the
/// module docs for whose field this reuses and why it is asked of it.
///
/// # Safety
///
/// `settings` must be NULL or a valid `CamelSettings`. It is only read from,
/// and nothing outlives the call.
pub unsafe fn uses_api_token(settings: *mut CamelSettings) -> bool {
    // SAFETY: the contract above is `network`'s own.
    let Some(network) = (unsafe { network(settings) }) else {
        return false;
    };
    // SAFETY: `network` implements the interface, which is `network`'s own
    // guarantee; the `dup_` accessor returns a g_malloc'd copy this call
    // frees via `take_string` rather than a pointer into storage another
    // thread may replace.
    let method = unsafe { take_string(camel_network_settings_dup_auth_mechanism(network)) };
    jmap_backend_core::api_token::method_is_api_token(method.as_deref())
}
