// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether an account authenticates with a manually pasted API token, sent as
//! `Authorization: Bearer`. This is a third choice beside
//! [`crate::oauth2`]'s OAuth 2.0 and [`crate::connect::credentials`]'s Basic.
//!
//! ## Why this exists
//!
//! Fastmail is the concrete case. Its JMAP API accepts a Bearer token from
//! the account's "API tokens" settings page and rejects the account login
//! password. Without this, OAuth 2.0 is the only way to reach it.
//! A Bearer-token provider needs an entry point that is neither "type a
//! password" nor "consent through a browser". Thus `jmap-config`'s
//! `Authentication` combo has a third choice that writes
//! [`API_TOKEN_METHOD`] into `[Authentication] Method`. The connect paths
//! read that field with this module.
//!
//! ## Why the token uses the password prompt, not a new one
//!
//! EDS's credentials provider sends a method that it has not registered as
//! an OAuth 2.0 alias to `e_soup_session_maybe_prepare_basic_auth`'s
//! ordinary prompt. This is the ordinary "Enter Password" dialog, which
//! writes into `E_SOURCE_CREDENTIAL_PASSWORD`. Thus the token is the text
//! the user pasted into that dialog. Nothing here prompts for it a second
//! time or adds a new credential kind to EDS. The token uses the same
//! libsecret entry as Basic, and no new UI is necessary.

use std::ffi::CStr;

use eds_sys::ESource;

use crate::source::read_auth;

/// The literal [`jmap-config`](../../jmap_config/index.html)'s "API Token"
/// combo entry writes to `[Authentication] Method`. Unlike
/// [`crate::oauth2::OAUTH2_METHOD`], EDS does not recognise this spelling.
/// It must only be different from all OAuth 2.0 aliases and from the
/// `ESourceAuthentication` default (`"none"`). Thus [`method_is_api_token`]
/// never confuses them.
pub const API_TOKEN_METHOD: &CStr = c"bearer";

/// Whether `[Authentication] Method` names the API-token method. The module
/// docs tell whose choice this reads.
pub fn method_is_api_token(method: Option<&str>) -> bool {
    method
        == Some(
            API_TOKEN_METHOD
                .to_str()
                .expect("API_TOKEN_METHOD is an ASCII literal"),
        )
}

/// [`method_is_api_token`] applied to a source. `connect_with` uses it
/// through this function. This is the API-token sibling of
/// [`crate::oauth2::source_uses_oauth2`].
///
/// # Safety
///
/// `source` must be a valid `ESource` (the one EDS gave the backend). The
/// function only reads from it, and nothing outlives the call.
pub unsafe fn source_uses_api_token(source: *mut ESource) -> bool {
    // `read_auth` adds no `[Authentication]` group to the account source.
    // SAFETY: the source is valid by this function's contract.
    let method = unsafe { read_auth(source) }.method;

    method_is_api_token(method.as_deref())
}
