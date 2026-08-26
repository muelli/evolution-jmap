// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether an account authenticates with OAuth 2.0, and the access token if it
//! does.
//!
//! ## The rule is EDS's, transcribed rather than invented
//!
//! Nothing here decides *policy*. `e_soup_session_setup_message_credentials`
//! (evolution-data-server 3.52.3, `src/libedataserver/e-soup-session.c`) is the
//! function every WebDAV, CalDAV and CardDAV account in Evolution
//! authenticates through, and it chooses like this:
//!
//! ```c
//! if (g_strcmp0 (auth_method, "OAuth2") == 0 ||
//!     e_oauth2_services_is_oauth2_alias_static (auth_method)) {
//!         success = e_soup_session_maybe_prepare_bearer_auth (…);
//! } else if (…) {
//! } else if (user && *user) {
//!         success = e_soup_session_maybe_prepare_basic_auth (…);
//! }
//! ```
//!
//! …and the bearer branch's token is `e_source_get_oauth2_access_token_sync`.
//! [`method_is_oauth2`] and [`access_token`] are those two lines for JMAP. The
//! rule is copied rather than approximated because it is the one an account
//! *written* by anything else in Evolution — the setup UI, a hand-edited
//! keyfile, a future EDS release — will have been written to satisfy, and a
//! JMAP account that read the same keyfield differently would be an account
//! Evolution thinks is OAuth2 and this code asks for a password.
//!
//! The `"OAuth2"` literal and the alias lookup are two different questions and
//! both are asked. `"OAuth2"` is the generic spelling, meaning "whichever
//! registered service claims this source"; an alias is a *particular* service's
//! [`name`][name] written into the field, which is what this project's own
//! setup writes (`jmap_config::oauth2_service::NAME`, `"JMAP"`). The alias
//! lookup is a live query against the registered services rather than a
//! constant list, so it answers yes for our service exactly when
//! `module-jmap-backend.so` has registered it — which is the same condition
//! under which the token could be fetched at all.
//!
//! [name]: ../../jmap_config/oauth2_service/constant.NAME.html
//!
//! ## Why the services object is held rather than made per question
//!
//! The one transcription liberty taken with EDS's rule above is which of the
//! two spellings of the alias lookup is called.
//! `e_oauth2_services_is_oauth2_alias_static()` — the one
//! `e_soup_session_setup_message_credentials` uses — creates an
//! `EOAuth2Services`, queries it and drops that reference again on every call,
//! and its own documentation names the precondition that makes that safe:
//!
//! > The #EOAuth2Services is implemented as a singleton, thus it won't be much
//! > trouble, **as long as there is something else having created one
//! > instance.**
//!
//! Where nothing else has, the reference it drops is the last one, and two
//! threads asking at once race: one is inside `oauth2_services_dispose`, which
//! frees `priv->services` without clearing the field, while the other is inside
//! `oauth2_services_constructor`, which still sees the not-yet-cleared
//! `services_singleton` and takes a reference to it — a legal GObject
//! resurrection of an object whose service list has just been freed. The
//! resurrected instance is then walked, and a dangling `EOAuth2Service` is
//! dereferenced. That is a SIGSEGV in whichever process asked, which for a
//! collection backend is `evolution-source-registry` and so every account in
//! the session.
//!
//! Every real Evolution process happens to satisfy the precondition —
//! `e_source_registry_init()` holds an `EOAuth2Services` for the registry's
//! lifetime — so this is a crash nothing reaches by accident *today*. This
//! module holds one on purpose instead: a single instance, created on the first
//! question and never released, which is EDS's precondition met deliberately
//! rather than inherited from whoever else happens to be in the process.
//! `e_oauth2_services_is_oauth2_alias()` is then the same lookup `_static`
//! would have made, minus the create-and-destroy around it.
//!
//! **What holding one does *not* change is which services are found.** An
//! `EOAuth2Services` loads its extensions once, at construction
//! (`e_extensible_load_extensions`), so which services answer depends on what
//! had registered one by the time the instance was built. In a process that
//! holds a registry, the instance is the registry's — and `_static` was already
//! returning that same one, never getting as far as constructing anything of
//! its own, because the singleton was alive throughout. Taking one more
//! reference to it changes nothing about when it was built. Only where nothing
//! else holds one does this fix the instance's age, and that is exactly where
//! it fixes the crash. `e_extensible_reload_extensions()` is deliberately *not*
//! called to re-widen that: it mutates the extension array under no lock of its
//! own, so calling it per question would put back a race of the same kind this
//! removes.
//!
//! ## What is deliberately not re-checked here
//!
//! **That the connection is encrypted.** EDS's own rule pre-fills credentials
//! only on `https`, and this code sends a bearer token over whatever origin
//! [`SourceConfig`](crate::source::SourceConfig) resolved — but that is not a
//! weaker rule, it is the same rule enforced one layer up:
//! [`source::origin`](crate::source::origin) already refuses to build a
//! non-TLS origin for any host that is not loopback. So a token can only ever
//! leave this process in clear over a connection to this machine, which is the
//! `jmap-mockd` development case, and adding a second TLS test here would be a
//! rule that exists twice and gets corrected once.
//!
//! **The token's lifetime.** `e_source_get_oauth2_access_token_sync` also
//! answers how many seconds the token is good for, and it is dropped on the
//! floor: a connect fetches a token, uses it for that connect, and the next
//! connect asks again. EDS refreshes an expired token inside that call, so the
//! number is only of use to something that holds a connection open across the
//! expiry and re-authenticates in place — which no JMAP backend here does.

use std::ffi::CString;
use std::ptr;
use std::sync::OnceLock;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, EOAuth2Services, ESource, ESourceAuthentication,
    e_oauth2_services_is_oauth2_alias, e_oauth2_services_new, e_source_authentication_get_method,
    e_source_authentication_get_type, e_source_get_oauth2_access_token_sync,
};
use gio_sys::{G_IO_ERROR_NOT_FOUND, GCancellable, g_dbus_error_quark, g_io_error_quark};
use glib_sys::{GError, GFALSE, g_error_free, g_free};

use crate::connect::ConnectError;
use crate::i18n::{translate, translate_with};
use crate::marshal::{extension_if_present, read_string};

/// The generic spelling of "this source authenticates with OAuth 2.0", as
/// opposed to the name of one particular service.
pub const OAUTH2_METHOD: &str = "OAuth2";

/// The process's `EOAuth2Services`, kept alive for as long as the process is —
/// see the module docs on why holding one is part of asking the question
/// safely.
///
/// A raw pointer rather than a wrapper with a `Drop`, deliberately: there is no
/// point in the process's life at which releasing this would be an improvement,
/// and a release is exactly the thing whose absence makes the singleton safe to
/// query. Everything below only ever reads it.
struct Services(*mut EOAuth2Services);

// SAFETY: the pointer is only ever handed back to EDS's own accessors, and the
// ones this module calls take the instance's `property_lock` around every read
// of the service list (`e-oauth2-services.c`). The pointer itself is written
// once, under `OnceLock`, and read-only afterwards.
unsafe impl Send for Services {}
// SAFETY: as `Send`.
unsafe impl Sync for Services {}

static SERVICES: OnceLock<Services> = OnceLock::new();

/// The held `EOAuth2Services`, created on the first question.
///
/// `OnceLock` is what closes the window the module docs describe rather than
/// merely narrowing it: the first thread to ask creates the instance while
/// every other waits, so no second thread can be part-way through a
/// create-or-destroy of the singleton while this one runs. From then on this
/// module holds a reference, so nothing else in the process can drop the last
/// one either — including EDS's own transient `ESourceRegistry` inside
/// [`access_token`], which is only ever reached *after* a question has been
/// asked and so after this has run.
fn services() -> *mut EOAuth2Services {
    // SAFETY: no arguments; the reference this returns is transfer-full and is
    // deliberately never given back.
    SERVICES
        .get_or_init(|| Services(unsafe { e_oauth2_services_new() }))
        .0
}

/// Whether `[Authentication] Method` names OAuth 2.0 — see the module docs for
/// whose rule this is.
///
/// `None` is a source with no `[Authentication]` group at all. It is not the
/// same as `Some("none")`, which is what a source that *has* the group reads
/// back as when nobody set a method — `ESourceAuthentication:method` has no
/// unset state — and both are equally not OAuth 2.0.
pub fn method_is_oauth2(method: Option<&str>) -> bool {
    let Some(method) = method else {
        return false;
    };
    if method == OAUTH2_METHOD {
        return true;
    }
    // An interior NUL cannot be an authentication method: EDS stores the field
    // as a C string, so a value containing one could never have been written
    // through `e_source_authentication_set_method` in the first place. Refusing
    // is the safe direction — it answers "not OAuth2" and the account falls
    // back to the password path, rather than truncating and possibly matching a
    // service whose name is a prefix of the string.
    let Ok(method) = CString::new(method) else {
        return false;
    };
    // An `EOAuth2Services` that could not be constructed is one with no
    // registered services to match against, which is the same answer an empty
    // one gives — and the same safe direction as the interior-NUL case above:
    // the account falls back to the password path rather than to a token
    // nothing could have issued.
    let services = services();
    if services.is_null() {
        return false;
    }
    // SAFETY: a live `EOAuth2Services` this module holds a reference to for the
    // process's lifetime, and a NUL-terminated string valid for the call. The
    // function applies EDS's own guard against "none"/"plain/password"/the
    // empty string, locks the instance around the lookup, and takes nothing of
    // ours.
    unsafe { e_oauth2_services_is_oauth2_alias(services, method.as_ptr()) != GFALSE }
}

/// [`method_is_oauth2`] asked of a source, which is how `connect_sync` reaches
/// it.
///
/// # Safety
///
/// `source` must be a valid `ESource` — the one EDS handed the backend. It is
/// only read from, and nothing outlives the call.
pub unsafe fn source_uses_oauth2(source: *mut ESource) -> bool {
    // As everywhere in this crate an extension is looked up by name: the
    // lookup walks the registered children of `E_TYPE_SOURCE_EXTENSION`, so a
    // type nothing has referenced yet is one it cannot find.
    // SAFETY: no arguments, and the type system initialises itself.
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

    method_is_oauth2(method.as_deref())
}

/// Whether `error` is a failure *underneath* EDS's own OAuth 2.0 reasoning —
/// the secret store itself, or the D-Bus call that reaches it — rather than
/// one of the outcomes `evolution-data-server`'s own OAuth 2.0 code
/// deliberately sets. Every deliberate outcome that code sets — nobody has
/// ever consented, a refresh attempt was rejected, this source has no
/// registered `EOAuth2Service` at all — is set in the `G_IO_ERROR` domain
/// (`e-oauth2-service.c`, `e-source.c`, `e-server-side-source.c`), whichever
/// code; all of them are answered [`ConnectError::OAuth2`]/`REQUIRED`
/// unchanged, on purpose — even "no registered service", which is what
/// every `ESource` this crate's own tests build (never backed by a real
/// registry) hits, and those tests already pin `REQUIRED` for it.
///
/// Confirmed by hand, not assumed (`docs/ROADMAP.md` item 17's own ask): with
/// `org.freedesktop.secrets` unable to start (a broken D-Bus activation, a
/// real, reproducible shape of a dead secret store), the exact call this
/// function classifies for answers `g_dbus_error_quark()`/
/// `G_DBUS_ERROR_SPAWN_EXEC_FAILED` — a different *domain*, which is the one
/// distinction drawn here rather than guessing at every code EDS's own
/// machinery might set.
///
/// **Does not catch every "the store is locked" case.** A collection that
/// exists but is locked, whose unlock prompt cannot be shown (no display,
/// or the user dismisses it), answers libsecret's search as "not found" —
/// by that API's own documented contract a dismissed or unshowable prompt
/// is not an error — so it is indistinguishable here from "nobody has ever
/// consented". No GError reaches this function to classify in that case;
/// there is nothing this function could do differently, and
/// [`is_secret_not_found`] plus [`crate::secret_store`] are what answer it
/// instead.
///
/// # Safety
///
/// `error` must be a valid, non-NULL `GError`.
unsafe fn is_secret_store_failure(error: *const GError) -> bool {
    // SAFETY: the caller's contract; `g_dbus_error_quark` takes no arguments.
    unsafe { (*error).domain == g_dbus_error_quark() }
}

/// Whether `error` is EDS's "this account has no stored secret" — the one
/// outcome a *locked* keyring is indistinguishable from, and so the only one
/// worth asking the secret store itself about.
///
/// `eos_lookup_token_sync` (`e-oauth2-service.c`) sets exactly this for both
/// "nobody has ever consented" and "the lookup came back empty", which is
/// what a collection whose unlock prompt cannot be shown produces — see
/// [`is_secret_store_failure`] above and [`crate::secret_store`] for why the
/// two arrive here identical.
///
/// Domain **and** code, not message text, per `docs/ROADMAP.md` item 17's own
/// ask. The narrowness is load-bearing rather than tidiness: EDS's other
/// deliberate `G_IO_ERROR`-domain outcomes must keep going to
/// [`ConnectError::OAuth2`]/`REQUIRED` untouched, and `G_IO_ERROR_NOT_SUPPORTED`
/// — what a bare `ESource` with no registry behind it answers, which is every
/// `ESource` this crate's own tests build — is the one that would otherwise
/// make those tests depend on whether the machine running them has a locked
/// keyring.
///
/// # Safety
///
/// `error` must be a valid, non-NULL `GError`.
unsafe fn is_secret_not_found(error: *const GError) -> bool {
    // SAFETY: the caller's contract; `g_io_error_quark` takes no arguments.
    unsafe { (*error).domain == g_io_error_quark() && (*error).code == G_IO_ERROR_NOT_FOUND }
}

/// The OAuth 2.0 access token to send this account's requests as, from
/// whichever `EOAuth2Service` claims `source`.
///
/// This is where the refresh happens: EDS looks the account's refresh token up
/// in libsecret and exchanges it for an access token inside this call, so what
/// comes back is good now. A failure is [`ConnectError::OAuth2`] when it is
/// "nobody has consented to this account yet" or "the refresh was rejected" —
/// see that variant on why it asks Evolution to authenticate rather than
/// discard what it has — and [`ConnectError::SecretStore`] when it is
/// something underneath that decision instead, per
/// `is_secret_store_failure` below.
///
/// # Safety
///
/// `source` must be a valid `ESource` and `cancellable` NULL or a valid
/// `GCancellable` — which is what an EDS vfunc receives. Nothing here outlives
/// the call.
pub unsafe fn access_token(
    source: *mut ESource,
    cancellable: *mut GCancellable,
) -> Result<String, ConnectError> {
    let account_id = unsafe { read_string(eds_sys::e_source_get_uid(source)) };
    tracing::debug!(?account_id, "fetching OAuth 2.0 access token via EDS");
    let mut token = ptr::null_mut();
    let mut expires_in = 0;
    let mut error: *mut GError = ptr::null_mut();

    // SAFETY: a valid source and cancellable by this function's contract, and
    // three writable out-parameters. The token comes back as a GLib allocation
    // this call owns, and the GError likewise.
    let ok = unsafe {
        e_source_get_oauth2_access_token_sync(
            source,
            cancellable,
            &mut token,
            &mut expires_in,
            &mut error,
        )
    };

    if ok == GFALSE || token.is_null() {
        // SAFETY: `error` is NULL or a GError this call owns; reading its
        // message and classifying its domain/code borrow fields the struct
        // owns, and freeing it afterwards is what the out-parameter contract
        // asks for.
        let (message, store_failure, secret_not_found) = unsafe {
            let outcome = if error.is_null() {
                (None, false, false)
            } else {
                (
                    read_string((*error).message),
                    is_secret_store_failure(error),
                    is_secret_not_found(error),
                )
            };
            if !error.is_null() {
                g_error_free(error);
            }
            outcome
        };
        tracing::debug!(
            ?account_id,
            ?message,
            store_failure,
            secret_not_found,
            "failed to obtain OAuth 2.0 access token"
        );

        if store_failure {
            return Err(ConnectError::SecretStore(translate_with(
                // TRANSLATORS: %1$s is EDS's own message for why the
                // account's secret store (keyring) could not be reached.
                c"the account's secret store (keyring) could not be reached: %1$s",
                &[&message.unwrap_or_else(|| translate(c"no further detail was given"))],
            )));
        }

        // "No stored secret" is the one answer EDS gives for two different
        // situations — nobody has consented yet, and the keyring holding the
        // consent is locked — so it is the one answer worth a second
        // question. Asked here, *after* the fetch has already failed and
        // only to classify that failure, which is what makes the inherent
        // race harmless: a store unlocked between the fetch and this call
        // answers "not locked" and the account gets the consent window it
        // would have got anyway, while one locked in that window is a store
        // that really is locked. Nothing is skipped or retried on the
        // strength of the answer, so it can only ever change the message.
        //
        // `Some(true)` and nothing else: a secret service that could not be
        // reached, or that is not there at all, answers `None`, and a
        // machine with no keyring must keep behaving exactly as before —
        // see `crate::secret_store`.
        if secret_not_found && crate::secret_store::default_collection_is_locked() == Some(true) {
            tracing::debug!(
                ?account_id,
                "the secret store is locked; not asking for consent"
            );
            return Err(ConnectError::SecretStore(translate(
                // TRANSLATORS: shown instead of a fresh sign-in window when
                // the account's stored token cannot be read because the
                // system keyring is locked, which signing in again cannot
                // fix.
                c"the login keyring is locked, so this account's OAuth 2.0 token can be neither read nor stored; unlock the keyring and try again",
            )));
        }

        // A failure with no GError should not happen — EDS sets one on every
        // path — but a NULL message is not worth turning into a panic in a
        // backend, so it becomes a sentence that says exactly what is known.
        return Err(ConnectError::OAuth2(message.unwrap_or_else(|| {
            translate(
                // TRANSLATORS: shown when EDS refused an OAuth 2.0 account's
                // access token but did not say why.
                c"the account has no OAuth 2.0 access token",
            )
        })));
    }

    // SAFETY: non-NULL on success, a NUL-terminated GLib allocation this scope
    // owns; copied out and then freed, as `(transfer full)` requires.
    let value = unsafe {
        let value = read_string(token);
        g_free(token.cast());
        value
    };

    // `read_string` answers `None` for NULL only, which `token.is_null()`
    // already ruled out; an empty token is a token the server would reject, and
    // saying so here is better than sending `Authorization: Bearer `.
    match value {
        Some(value) if !value.is_empty() => {
            tracing::debug!(?account_id, expires_in, "obtained OAuth 2.0 access token");
            Ok(value)
        }
        _ => {
            tracing::debug!(?account_id, "empty OAuth 2.0 access token received");
            Err(ConnectError::OAuth2(translate(
                // TRANSLATORS: shown when EDS handed back an OAuth 2.0 access
                // token that was empty rather than absent.
                c"the OAuth 2.0 service returned an empty access token",
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use gio_sys::{
        G_DBUS_ERROR_SPAWN_EXEC_FAILED, G_IO_ERROR_NOT_FOUND, G_IO_ERROR_NOT_SUPPORTED,
        g_dbus_error_quark, g_io_error_quark,
    };
    use glib_sys::g_error_new_literal;

    use super::*;

    /// Builds a real `GError` rather than a fake struct: [`is_secret_store_failure`]
    /// reads its `domain` field directly, so the test should exercise the same
    /// GLib allocation the production code does, not a hand-rolled substitute.
    fn error(domain: glib_sys::GQuark, code: i32) -> *mut GError {
        let message = CString::new("boom").unwrap();
        // SAFETY: a valid domain and a NUL-terminated message; the result is
        // freed by every caller below.
        unsafe { g_error_new_literal(domain, code, message.as_ptr()) }
    }

    /// `G_IO_ERROR`, whichever code, is the domain EDS's own OAuth 2.0 code
    /// deliberately sets every outcome in — including "no registered
    /// service", not just "no grant yet" — and none of it is a secret-store
    /// failure. The `jmap-backend-core/tests/oauth2.rs` integration test
    /// exercises the "no registered service" shape end to end and expects
    /// `REQUIRED`; this pins the unit underneath it.
    #[test]
    fn the_g_io_error_domain_is_never_a_secret_store_failure() {
        for code in [G_IO_ERROR_NOT_FOUND, G_IO_ERROR_NOT_SUPPORTED] {
            let error = error(unsafe { g_io_error_quark() }, code);
            assert!(
                !unsafe { is_secret_store_failure(error) },
                "expected code {code} to not be a secret-store failure"
            );
            unsafe { g_error_free(error) };
        }
    }

    /// The real, reproduced shape of a dead secret store: a broken
    /// `org.freedesktop.secrets` D-Bus activation answers
    /// `g_dbus_error_quark()`/`G_DBUS_ERROR_SPAWN_EXEC_FAILED`, confirmed by
    /// hand against a real `gnome-keyring-daemon` whose `.service` file
    /// pointed at a missing binary — a different domain from `G_IO_ERROR`,
    /// and this must not open a fresh consent window.
    #[test]
    fn a_broken_secret_service_is_a_secret_store_failure() {
        let error = error(
            unsafe { g_dbus_error_quark() },
            G_DBUS_ERROR_SPAWN_EXEC_FAILED,
        );
        assert!(unsafe { is_secret_store_failure(error) });
        unsafe { g_error_free(error) };
    }

    /// The shape `eos_lookup_token_sync` gives both for "nobody has ever
    /// consented" and for a lookup a locked collection answered empty — the
    /// only outcome [`access_token`] asks the secret store itself about.
    #[test]
    fn a_missing_secret_is_the_one_ambiguous_outcome() {
        let error = error(unsafe { g_io_error_quark() }, G_IO_ERROR_NOT_FOUND);
        assert!(unsafe { is_secret_not_found(error) });
        unsafe { g_error_free(error) };
    }

    /// `G_IO_ERROR_NOT_SUPPORTED` is what an `ESource` with no registry
    /// behind it answers — every `ESource` this crate's own tests build, and
    /// `tests/oauth2.rs`/`jmap-backend-collection`'s `tests/authenticate.rs`
    /// both pin it to `REQUIRED`. It must not reach the secret store
    /// question, or those tests would start depending on whether the machine
    /// running them has a locked keyring.
    #[test]
    fn no_registered_service_is_not_a_missing_secret() {
        let error = error(unsafe { g_io_error_quark() }, G_IO_ERROR_NOT_SUPPORTED);
        assert!(!unsafe { is_secret_not_found(error) });
        unsafe { g_error_free(error) };
    }

    /// Domain and code, not code alone: `G_IO_ERROR_NOT_FOUND` is the
    /// integer 1, which is a perfectly ordinary code in every other error
    /// domain too — including the D-Bus one this module already classifies
    /// separately.
    #[test]
    fn a_missing_secret_is_recognised_by_domain_not_by_code_alone() {
        let error = error(unsafe { g_dbus_error_quark() }, G_IO_ERROR_NOT_FOUND);
        assert!(!unsafe { is_secret_not_found(error) });
        unsafe { g_error_free(error) };
    }
}
