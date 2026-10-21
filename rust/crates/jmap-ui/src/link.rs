// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The connection from the `ESource` of an account to a connected client.
//! All features use this module.
//!
//! The vacation page connects from the collection source of the editor.
//! Scheduled send connects from the parent of the transport source.
//! All functions here block on the network or on libsecret. Thus they run
//! only on worker threads ([`crate::dispatch`], or a `GTask` thread for a
//! page submit).
//!
//! The connection procedure is the same as in the backends. `SourceConfig`
//! reads `[Authentication]`/`[Security]` from the source. The credentials
//! have three branches on the authentication method, as
//! `jmap-backend-core::connect` describes:
//!
//! - OAuth 2.0 through `e_source_get_oauth2_access_token_sync`. This works
//!   in the Evolution shell because `jmap_config::module::load` registers
//!   the `EOAuth2Service` there.
//! - An API token from the stored secret.
//! - A password from the stored secret.
//!
//! This module does *not* use the prompt mechanism of EDS. A settings page
//! has no `ESourceCredentialsProvider`. Thus, for an account with no stored
//! secret, this module reports an error and does not prompt.

use std::ffi::{CStr, c_char};
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, ESource, ESourceAuthentication,
    e_source_authentication_get_method, e_source_lookup_password_sync,
};
use glib_sys::{GError, g_error_free, g_free};
use jmap_backend_core::i18n::{translate, translate_with};
use jmap_backend_core::marshal::{extension_if_present, read_string};
use jmap_backend_core::{api_token, connect, oauth2, retry, source};
use jmap_client::{Client, Credentials};

use crate::session_cache::AccountFeatures;

/// A [`jmap_client::Error`] as a text for a user.
///
/// The `Display` of `Error` is for a log. "HTTP 401" is correct but tells a
/// user nothing, and a user reported this problem. This function gives
/// separate texts for the errors that a user can correct. For all other
/// errors, it uses the `Display` text, which names the layer that failed.
pub fn describe(error: &jmap_client::Error) -> String {
    match error {
        // Only after a token refresh failed (see `AccountLink::call`): the user must sign in again.
        jmap_client::Error::Http { status: 401, .. } => translate(
            c"the server rejected the account's credentials — it may need to be signed in again",
        ),
        jmap_client::Error::Http { status: 403, .. } => {
            translate(c"the server refused: this account is not allowed to do that")
        }
        jmap_client::Error::Transport(detail) => translate_with(
            // TRANSLATORS: %1$s is the network error the transport reported.
            c"the server could not be reached: %1$s",
            &[detail],
        ),
        jmap_client::Error::Set(set_error) => {
            let detail = set_error
                .description
                .clone()
                .unwrap_or_else(|| set_error.error_type.clone());
            // TRANSLATORS: %1$s is the reason the mail server gave.
            translate_with(c"the server rejected the change: %1$s", &[&detail])
        }
        // The token is not rejected. The transport did not send the
        // credentials on a redirect to a different host (RFC 7235), and that
        // host answered 401. A new sign-in does not correct this.
        jmap_client::Error::RedirectCredentialsWithheld { requested, followed } => translate_with(
            // TRANSLATORS: %1$s and %2$s are the URLs the server redirected
            // between; they differ in host.
            c"the server at %1$s redirected to %2$s, a different host, so its credentials were not sent there — check the account's host setting",
            &[requested, followed],
        ),
        other => other.to_string(),
    }
}

/// A connected client and the session data about its account.
///
/// This is the connection of a page to the server. The load and all later
/// submits share it through an `Arc`.
pub struct AccountLink {
    pub client: Client,
    pub features: AccountFeatures,
    /// The source of this connection. It stays alive so that
    /// [`AccountLink::call`] can refresh and install an expired OAuth 2.0
    /// access token. `None` for a method whose credential does not expire
    /// (a stored password, an API token). For such a method, a 401 means
    /// that the credential is wrong, not old.
    refreshable: Option<SendSource>,
}

/// An owned `ESource` reference that can move to a worker thread, or stay in
/// an [`AccountLink`] that threads share. The drop releases the reference, on
/// the thread that drops it.
///
/// `ESource` is a GObject, so its reference count is atomic. The getters
/// that this crate calls take the property lock of the source. The EDS
/// backends read their sources from worker threads on the same basis.
pub(crate) struct SendSource(*mut ESource);

// SAFETY: see the doc of the type. Reference counting is thread-safe, and
// EDS itself makes each call through the pointer from its worker threads.
unsafe impl Send for SendSource {}
unsafe impl Sync for SendSource {}

impl SendSource {
    /// Take ownership of `source`, a reference that the caller owns.
    ///
    /// # Safety
    ///
    /// `source` must be a live `ESource`, and the caller must own the
    /// reference and give it away. The wrapper releases it one time, on drop.
    pub(crate) unsafe fn adopt(source: *mut ESource) -> Self {
        Self(source)
    }

    /// The source. It stays alive while `self` is alive.
    pub(crate) fn as_ptr(&self) -> *mut ESource {
        self.0
    }
}

impl Drop for SendSource {
    fn drop(&mut self) {
        // SAFETY: the reference that `adopt` took, released one time.
        unsafe { gobject_sys::g_object_unref(self.0.cast()) };
    }
}

impl AccountLink {
    /// A link on a client that is already connected, with no token to refresh.
    ///
    /// A test against the mock uses this (no `ESource`, no OAuth 2.0). By
    /// design, this is the only constructor other than [`connect_account`].
    pub fn without_refresh(client: Client, features: AccountFeatures) -> Self {
        Self {
            client,
            features,
            refreshable: None,
        }
    }

    /// Run one JMAP call. If the server answers 401, refresh the OAuth 2.0
    /// access token of the account and try the call again one time.
    ///
    /// The EDS backends use the same control flow
    /// ([`jmap_backend_core::retry::retry_once_after`]). It lets a session
    /// continue after its token expires. Fastmail access tokens last
    /// approximately one hour. An account editor or a composer can stay open
    /// much longer, and the stored refresh token stays valid. Without this
    /// method, a save after one hour fails with a 401. Before this method
    /// existed, that failure occurred.
    ///
    /// All server calls in this crate go through this method. Do not call
    /// `link.client` directly: this method exists to prevent that bug.
    pub fn call<T>(
        &self,
        mut op: impl FnMut(&Client) -> Result<T, jmap_client::Error>,
    ) -> Result<T, jmap_client::Error> {
        retry::retry_once_after(
            || op(&self.client),
            |error| matches!(error, jmap_client::Error::Http { status: 401, .. }),
            || self.refresh_token(),
        )
    }

    /// Get a new access token and install it on the client.
    ///
    /// `false` when there is no token to refresh or the refresh failed.
    /// Then the caller reports the original 401.
    fn refresh_token(&self) -> bool {
        let Some(source) = self.refreshable.as_ref() else {
            return false;
        };
        // SAFETY: a live source, which `SendSource` keeps referenced for the
        // life of this link. No cancellable: this thread can block.
        match unsafe { oauth2::access_token(source.as_ptr(), ptr::null_mut()) } {
            Ok(token) => {
                tracing::debug!("refreshed the UI module's OAuth 2.0 access token");
                self.client.set_credentials(Credentials::bearer(token));
                true
            }
            Err(failure) => {
                tracing::debug!(%failure, "could not refresh the access token");
                false
            }
        }
    }
}

/// Connect as the account that `source` configures.
///
/// `source` is the *collection* source of the editor if the account has one.
/// For a JMAP account, `[Authentication]` and `[Security]` are in that
/// source. Otherwise, `source` is the mail-account source.
///
/// The error is the text for the user. Most errors are the same translated
/// messages that the backends put in their `GError`s.
///
/// # Safety
///
/// `source` must be a valid `ESource`, and the caller must keep it alive for
/// the duration of the call. This function blocks: never call it on the
/// main loop.
pub unsafe fn connect_account(source: *mut ESource) -> Result<AccountLink, String> {
    // SAFETY: `source` is valid per this function's contract.
    let config = unsafe { source::SourceConfig::from_source(source) }
        .map_err(|failure| gerror_message(failure.to_gerror()))?;

    // SAFETY: as above. The extension pointer belongs to the source.
    let method = unsafe {
        extension_if_present::<ESourceAuthentication>(source, E_SOURCE_EXTENSION_AUTHENTICATION)
    }
    .and_then(|authentication| {
        // SAFETY: a live extension of the source, which is still referenced.
        // The string belongs to the extension, and read_string copies it.
        unsafe { read_string(e_source_authentication_get_method(authentication)) }
    });

    let is_oauth2 = oauth2::method_is_oauth2(method.as_deref());
    let credentials = if is_oauth2 {
        // SAFETY: a valid source. No cancellable: this thread can block.
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

    // Only an OAuth 2.0 credential expires. For the other methods, a 401
    // means a wrong password or token, and a retry sends the same one.
    let refreshable = is_oauth2.then(|| {
        // SAFETY: `source` is valid for this call, by the contract of the
        // function. The link owns the reference taken here until it drops.
        unsafe { SendSource::adopt(gobject_sys::g_object_ref(source.cast()).cast()) }
    });

    Ok(AccountLink {
        client,
        features,
        refreshable,
    })
}

/// The password (or API token) that EDS stores for `source`, if any.
///
/// # Safety
///
/// `source` must be a valid `ESource`. This function blocks (libsecret over
/// D-Bus).
unsafe fn stored_password(source: *mut ESource) -> Option<String> {
    let mut raw: *mut c_char = ptr::null_mut();
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a valid source, no cancellable, and two writable out-parameters.
    // This call owns the returned string and the returned error.
    unsafe { e_source_lookup_password_sync(source, ptr::null_mut(), &mut raw, &mut error) };
    if !error.is_null() {
        // OAuth accounts usually have no stored secret. For other accounts,
        // log the error before the connection fails with its own text.
        // SAFETY: a GError that this call owns. `message` belongs to the struct.
        let message = unsafe { read_string((*error).message) };
        tracing::debug!(?message, "no stored password for the account source");
        // SAFETY: owned, freed one time.
        unsafe { g_error_free(error) };
    }
    if raw.is_null() {
        return None;
    }
    // SAFETY: a NUL-terminated string that this call owns. Copied, then freed.
    let password = unsafe { CStr::from_ptr(raw) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: the g_malloc string that e_source_lookup_password_sync gave.
    unsafe { g_free(raw.cast()) };
    (!password.is_empty()).then_some(password)
}

/// The message of one `GError` as the text for the user. This function frees
/// the error.
///
/// The callers here (`SourceError::to_gerror`, `ConnectError::to_gerror`)
/// always give an error. For a NULL, this function returns a placeholder text
/// and does not crash, because it runs in the Evolution process.
fn gerror_message(error: *mut GError) -> String {
    if error.is_null() {
        return translate(c"no further detail was given");
    }
    // SAFETY: a live GError that this function owns. The message belongs to
    // the struct, and the copy occurs before the free.
    let message = unsafe { read_string((*error).message) }
        .unwrap_or_else(|| translate(c"no further detail was given"));
    // SAFETY: owned, freed one time.
    unsafe { g_error_free(error) };
    message
}

#[cfg(test)]
mod describe_tests {
    use jmap_client::Error;

    use super::describe;

    #[test]
    fn a_401_reads_as_sign_in_again() {
        let error = Error::Http {
            status: 401,
            problem: None,
        };
        assert!(describe(&error).contains("signed in again"));
    }

    #[test]
    fn redirect_credentials_withheld_names_the_host_change_not_the_401() {
        let error = Error::RedirectCredentialsWithheld {
            requested: "https://api.fastmail.com./.well-known/jmap".into(),
            followed: "https://api.fastmail.com/jmap/session".into(),
        };
        let message = describe(&error);
        assert!(
            message.contains("api.fastmail.com"),
            "expected the message to name the hosts involved, got: {message}"
        );
        assert!(
            !message.contains("answered 401") && !message.contains("sign in"),
            "expected a redirect explanation, not the raw Display text or a \
             sign-in prompt (signing in again cannot fix a dropped redirect \
             header), got: {message}"
        );
    }
}

#[cfg(test)]
mod send_source_tests {
    use std::ffi::CString;
    use std::ptr;

    use eds_sys::e_source_new_with_uid;
    use gobject_sys::{GObject, g_object_ref, g_object_unref};

    use super::SendSource;

    fn ref_count(object: *mut GObject) -> u32 {
        // SAFETY: a live GObject. `ref_count` is its public counter, with
        // atomic updates. This reads it one time.
        unsafe { (*object).ref_count }
    }

    #[test]
    fn a_send_source_releases_its_reference_on_the_thread_that_drops_it() {
        let uid = CString::new("jmap-ui-send-source-test").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: the documented arguments: a NUL-terminated uid, no D-Bus
        // object, and a GError out-parameter.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        let object = source.cast::<GObject>();

        // SAFETY: a live source. The wrapper gets the new reference.
        let shared = unsafe { SendSource::adopt(g_object_ref(object.cast()).cast()) };
        assert_eq!(shared.as_ptr(), source);
        assert_eq!(ref_count(object), 2);

        std::thread::spawn(move || {
            let shared = shared;
            assert_eq!(ref_count(shared.as_ptr().cast()), 2);
        })
        .join()
        .expect("the worker thread panicked");

        assert_eq!(ref_count(object), 1, "the worker's drop released one ref");
        // SAFETY: the reference that `e_source_new_with_uid` returned.
        unsafe { g_object_unref(object.cast()) };
    }
}

#[cfg(test)]
mod gerror_message_tests {
    use std::ffi::CString;

    use glib_sys::{g_error_new_literal, g_quark_from_static_string};

    use super::gerror_message;

    #[test]
    fn the_text_is_the_message_of_the_error() {
        let message = CString::new("the host is not a URL").expect("no NUL in a literal");
        // SAFETY: a static domain string, and a NUL-terminated message that
        // the call copies. The new error goes to `gerror_message`, which frees it.
        let error = unsafe {
            g_error_new_literal(
                g_quark_from_static_string(c"jmap-ui-test".as_ptr()),
                1,
                message.as_ptr(),
            )
        };
        assert_eq!(gerror_message(error), "the host is not a URL");
    }

    #[test]
    fn a_null_error_gives_a_placeholder_text() {
        assert!(!gerror_message(std::ptr::null_mut()).is_empty());
    }
}
