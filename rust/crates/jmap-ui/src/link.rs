// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! From an account's `ESource` to a connected client — the piece every
//! feature shares: the vacation page connects from the editor's collection
//! source, scheduled send from the transport's parent. Everything here blocks
//! on the network or on libsecret, so it runs on worker threads only
//! ([`crate::dispatch`], or a `GTask` thread for a page submit).
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
use jmap_backend_core::i18n::{translate, translate_with};
use jmap_backend_core::marshal::{extension_if_present, read_string};
use jmap_backend_core::{api_token, connect, oauth2, retry, source};
use jmap_client::{Client, Credentials};

use crate::session_cache::AccountFeatures;

/// A [`jmap_client::Error`] as something worth putting in front of a person.
///
/// `Error`'s own `Display` is written for a log — "HTTP 401" is accurate and
/// tells a user nothing, which is exactly what one of them reported. The
/// cases split out here are the ones a user can act on; everything else falls
/// back to the underlying text, which at least names the layer that failed.
pub fn describe(error: &jmap_client::Error) -> String {
    match error {
        // Reached only when a refresh has already been tried and failed (see
        // `AccountLink::call`), so this really is "sign in again".
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
        other => other.to_string(),
    }
}

/// A connected client and what the session said about its account — the
/// page's handle on the server, shared between the load and every later
/// submit through an `Arc`.
pub struct AccountLink {
    pub client: Client,
    pub features: AccountFeatures,
    /// The source this connected as, kept alive so an expired OAuth 2.0
    /// access token can be refreshed and reinstalled — see
    /// [`AccountLink::call`]. `None` for a method whose credential does not
    /// expire (a stored password, an API token), where a 401 means the
    /// credential is wrong rather than old.
    refreshable: Option<RefreshableSource>,
}

/// An owned `ESource` reference an [`AccountLink`] may carry across threads.
///
/// `ESource` is a GObject: reference counting is atomic, and the getters used
/// here take the source's own property lock, which is the same ground the EDS
/// backends read their sources on from worker threads.
struct RefreshableSource(*mut ESource);

// SAFETY: see the type's own doc — refcounting is thread-safe and every call
// made through the pointer is one EDS itself makes off its worker threads.
unsafe impl Send for RefreshableSource {}
unsafe impl Sync for RefreshableSource {}

impl Drop for RefreshableSource {
    fn drop(&mut self) {
        // SAFETY: the reference `connect_account` took, released once.
        unsafe { gobject_sys::g_object_unref(self.0.cast()) };
    }
}

impl AccountLink {
    /// A link over an already-connected client, with nothing to refresh —
    /// what a test against the mock has (no `ESource`, no OAuth 2.0), and
    /// deliberately the only way to build one without [`connect_account`].
    pub fn without_refresh(client: Client, features: AccountFeatures) -> Self {
        Self {
            client,
            features,
            refreshable: None,
        }
    }

    /// Run one JMAP call, refreshing the account's OAuth 2.0 access token and
    /// retrying exactly once if the server answers 401.
    ///
    /// This is the control flow the EDS backends already share
    /// ([`jmap_backend_core::retry::retry_once_after`]) and the reason they
    /// survive a session outliving its token: Fastmail's access tokens last
    /// about an hour, an account editor or composer can sit open far longer,
    /// and the stored refresh token is still perfectly good. Without this a
    /// save an hour into the session fails with a bare 401 — which is exactly
    /// what it did before this existed.
    ///
    /// Every server call in this crate goes through here; a bare
    /// `link.client` call is the bug this method exists to prevent.
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

    /// Fetch a fresh access token and install it on the client. `false` when
    /// there is nothing to refresh or the refresh itself failed, which leaves
    /// the caller's original 401 to be reported.
    fn refresh_token(&self) -> bool {
        let Some(source) = self.refreshable.as_ref() else {
            return false;
        };
        // SAFETY: a live source, kept referenced by `RefreshableSource` for as
        // long as this link; no cancellable, this thread is ours to block.
        match unsafe { oauth2::access_token(source.0, ptr::null_mut()) } {
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

    let is_oauth2 = oauth2::method_is_oauth2(method.as_deref());
    let credentials = if is_oauth2 {
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

    // Only the OAuth 2.0 path has a credential that expires; for the others a
    // 401 means the password or token is wrong, and retrying would just ask
    // again with the same one.
    let refreshable = is_oauth2.then(|| {
        // SAFETY: `source` is valid for this call by the function's contract;
        // the reference taken here is owned by the link until it drops.
        RefreshableSource(unsafe { gobject_sys::g_object_ref(source.cast()) }.cast())
    });

    Ok(AccountLink {
        client,
        features,
        refreshable,
    })
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
