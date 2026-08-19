// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether an account authenticates with a manually pasted API token, sent as
//! `Authorization: Bearer` — a third choice beside [`crate::oauth2`]'s OAuth
//! 2.0 and [`crate::connect::credentials`]'s Basic.
//!
//! ## Why this exists
//!
//! Fastmail is the concrete case: its JMAP API accepts a Bearer token minted
//! on the account's "API tokens" settings page, and rejects the account
//! login password outright — there is no OAuth 2.0-free way to reach it
//! otherwise. A Bearer-token provider needs an entry point that is neither
//! "type a password" nor "consent through a browser", so
//! `jmap-config`'s `Authentication` combo gets a third choice that writes
//! [`API_TOKEN_METHOD`] into `[Authentication] Method`, and this module is
//! what the connect paths read that field with.
//!
//! ## Why the token rides the password prompt, not a new one
//!
//! A method EDS's credentials provider has never registered as an OAuth 2.0
//! alias falls through to `e_soup_session_maybe_prepare_basic_auth`'s
//! ordinary prompt — the ordinary "Enter Password" dialog, into
//! `E_SOURCE_CREDENTIAL_PASSWORD`. So the token is whatever the user pasted
//! into that dialog; nothing here prompts for it a second way or teaches EDS
//! a new credential kind, which is the same libsecret entry Basic already
//! uses and no new UI to build.

use std::ffi::CStr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, ESource, ESourceAuthentication,
    e_source_authentication_get_method, e_source_authentication_get_type,
};

use crate::marshal::{extension_if_present, read_string};

/// The literal [`jmap-config`](../../jmap_config/index.html)'s "API Token"
/// combo entry writes to `[Authentication] Method`. Unlike
/// [`crate::oauth2::OAUTH2_METHOD`] this is not a spelling EDS itself
/// recognises — it only has to be a string no OAuth 2.0 alias and no
/// `ESourceAuthentication` default (`"none"`) can equal, so [`method_is_api_token`]
/// never confuses the two.
pub const API_TOKEN_METHOD: &CStr = c"bearer";

/// Whether `[Authentication] Method` names the API-token method — see the
/// module docs for whose choice this reads.
pub fn method_is_api_token(method: Option<&str>) -> bool {
    method
        == Some(
            API_TOKEN_METHOD
                .to_str()
                .expect("API_TOKEN_METHOD is an ASCII literal"),
        )
}

/// [`method_is_api_token`] asked of a source, which is how `connect_with`
/// reaches it — the API-token sibling of
/// [`crate::oauth2::source_uses_oauth2`].
///
/// # Safety
///
/// `source` must be a valid `ESource` — the one EDS handed the backend. It is
/// only read from, and nothing outlives the call.
pub unsafe fn source_uses_api_token(source: *mut ESource) -> bool {
    // SAFETY: no arguments; registers the extension type the lookup below
    // needs, the same call `source_uses_oauth2` makes before its own lookup.
    unsafe { e_source_authentication_get_type() };

    // Asked before `e_source_get_extension`, which *creates* the extension it
    // cannot find: this is handed the user's own account source, and creating
    // a group on it to read a default out of it is a side effect on someone
    // else's object.
    // SAFETY: a valid source by this function's contract, and a header
    // constant for the name.
    let Some(auth) = (unsafe {
        extension_if_present::<ESourceAuthentication>(source, E_SOURCE_EXTENSION_AUTHENTICATION)
    }) else {
        return false;
    };

    // SAFETY: the extension exists, is owned by the source, and the string
    // the getter answers with is owned by the extension and outlives this
    // call.
    let method = unsafe { read_string(e_source_authentication_get_method(auth)) };

    method_is_api_token(method.as_deref())
}
