// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! From the editor's `ESource` to a connected client, and the two vacation
//! round trips. Everything here blocks on the network or on libsecret, so it
//! runs on worker threads only ([`crate::dispatch`], or a `GTask` thread for
//! the submit).
//!
//! The connection recipe is the backends' own, reused: `SourceConfig` reads
//! `[Authentication]`/`[Security]` off the source, and the credentials branch
//! three ways on the authentication method exactly as
//! `jmap-backend-core::connect` describes — OAuth 2.0 through
//! `e_source_get_oauth2_access_token_sync` (which works in Evolution's shell
//! because `jmap_config::module::load` registers the `EOAuth2Service` there),
//! an API token or a password out of the stored secret. What is *not* reused
//! is EDS's prompting machinery: a settings page has no
//! `ESourceCredentialsProvider`, so an account with nothing stored is
//! reported, not prompted for.

use std::ffi::{CStr, c_char};
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, ESource, ESourceAuthentication,
    e_source_authentication_get_method, e_source_lookup_password_sync,
};
use glib_sys::{GError, g_error_free, g_free};
use jmap_backend_core::i18n::translate;
use jmap_backend_core::marshal::{extension_if_present, read_string};
use jmap_backend_core::{api_token, connect, oauth2, source};
use jmap_client::{Client, Credentials};
use jmap_proto::mail::VacationResponse;
use serde_json::Value;

use crate::session_cache::AccountFeatures;

/// A connected client and what the session said about its account — the
/// page's handle on the server, shared between the load and every later
/// submit through an `Arc`.
pub struct AccountLink {
    pub client: Client,
    pub features: AccountFeatures,
}

/// Connect as the account `source` configures.
///
/// `source` is the editor's *collection* source when the account has one
/// (that is where `[Authentication]` and `[Security]` live for a JMAP
/// account), else its mail-account source.
///
/// The error is the user-facing text, most of it the same translated
/// messages the backends put in their `GError`s.
///
/// # Safety
///
/// `source` must be a valid `ESource`, kept alive by the caller for the
/// length of the call. Blocking; never on the main loop.
pub unsafe fn connect_account(source: *mut ESource) -> Result<AccountLink, String> {
    // SAFETY: `source` is valid per this function's contract.
    let config = unsafe { source::SourceConfig::from_source(source) }
        .map_err(|failure| gerror_message(failure.to_gerror()))?;

    // SAFETY: as above; the extension pointer is the source's own.
    let method = unsafe {
        extension_if_present::<ESourceAuthentication>(source, E_SOURCE_EXTENSION_AUTHENTICATION)
    }
    .and_then(|authentication| {
        // SAFETY: a live extension of the still-referenced source; the string
        // is the extension's own, copied by read_string.
        unsafe { read_string(e_source_authentication_get_method(authentication)) }
    });

    let credentials = if oauth2::method_is_oauth2(method.as_deref()) {
        // SAFETY: a valid source; no cancellable, the thread is ours to block.
        let token = unsafe { oauth2::access_token(source, ptr::null_mut()) }
            .map_err(|failure| gerror_message(failure.to_gerror()))?;
        Credentials::bearer(token)
    } else if api_token::method_is_api_token(method.as_deref()) {
        // SAFETY: a valid source, as above.
        connect::bearer_credentials(unsafe { stored_password(source) }.as_deref())
            .map_err(|failure| gerror_message(failure.to_gerror()))?
    } else {
        // SAFETY: a valid source, as above.
        connect::credentials(
            config.user.as_deref(),
            unsafe { stored_password(source) }.as_deref(),
        )
        .map_err(|failure| gerror_message(failure.to_gerror()))?
    };

    let client = source::connect(&config.target, config.rebase_urls, credentials)
        .map_err(|error| error.to_string())?;

    let features = AccountFeatures::from_session(client.session())
        .ok_or_else(|| translate(c"the session document names no usable mail account"))?;

    Ok(AccountLink { client, features })
}

/// The account's current autoresponder (`VacationResponse/get`).
pub fn load(link: &AccountLink) -> Result<VacationResponse, String> {
    link.client
        .vacation_response_get(&link.features.account_id)
        .map_err(|error| error.to_string())
}

/// Write the page's state back (`VacationResponse/set` update).
pub fn save(link: &AccountLink, patch: Value) -> Result<(), String> {
    link.client
        .vacation_response_update(&link.features.account_id, patch)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// The password (or API token) EDS has stored for `source`, if any.
///
/// # Safety
///
/// `source` must be a valid `ESource`. Blocking (libsecret over D-Bus).
unsafe fn stored_password(source: *mut ESource) -> Option<String> {
    let mut raw: *mut c_char = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a valid source, no cancellable, two writable out-parameters; the
    // string and the error both come back owned by this call.
    unsafe { e_source_lookup_password_sync(source, ptr::null_mut(), &mut raw, &mut error) };
    if !error.is_null() {
        // No secret stored is the ordinary case for OAuth accounts; anything
        // else is worth a trace before the connect fails with its own text.
        // SAFETY: a GError this call owns; message is the struct's own.
        let message = unsafe { read_string((*error).message) };
        tracing::debug!(?message, "no stored password for the account source");
        // SAFETY: owned, freed once.
        unsafe { g_error_free(error) };
    }
    if raw.is_null() {
        return None;
    }
    // SAFETY: a NUL-terminated string this call owns; copied, then freed.
    let password = unsafe { CStr::from_ptr(raw) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: the g_malloc'd string e_source_lookup_password_sync handed over.
    unsafe { g_free(raw.cast()) };
    (!password.is_empty()).then_some(password)
}

/// One `GError`'s message as the user-facing text, the error freed.
///
/// The producers here (`SourceError::to_gerror`, `ConnectError::to_gerror`)
/// always hand one back, but a NULL is answered with placeholder text rather
/// than a crash, since this runs in Evolution's own process.
fn gerror_message(error: *mut GError) -> String {
    if error.is_null() {
        return translate(c"no further detail was given");
    }
    // SAFETY: a live GError this function now owns; the message is the
    // struct's own, copied before the free.
    let message = unsafe { read_string((*error).message) }
        .unwrap_or_else(|| translate(c"no further detail was given"));
    // SAFETY: owned, freed once.
    unsafe { g_error_free(error) };
    message
}
