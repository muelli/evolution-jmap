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

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, ESource, ESourceAuthentication,
    e_source_authentication_dup_method, e_source_authentication_get_type,
};

use crate::marshal::{extension_if_present, take_string};

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
    // SAFETY: no arguments. It registers the extension type that the lookup
    // below needs, as `source_uses_oauth2` does before its own lookup.
    unsafe { e_source_authentication_get_type() };

    // Make sure that the extension exists before `e_source_get_extension`
    // runs: it *creates* an extension that it cannot find. This source is the
    // user's own account source. To create a group on it only to read a
    // default is a side effect on another object's state.
    // SAFETY: the source is valid by this function's contract, and the name
    // is a header constant.
    let Some(auth) = (unsafe {
        extension_if_present::<ESourceAuthentication>(source, E_SOURCE_EXTENSION_AUTHENTICATION)
    }) else {
        return false;
    };

    // SAFETY: the extension exists and the source owns it. The `dup_` getter
    // gives a copy (`transfer full`), so a setter on another thread cannot free it.
    let method = unsafe { take_string(e_source_authentication_dup_method(auth)) };

    method_is_api_token(method.as_deref())
}
