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
use gio_sys::GCancellable;
use glib_sys::{GError, GFALSE, g_error_free, g_free};

use crate::connect::ConnectError;
use crate::i18n::translate;
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

/// The OAuth 2.0 access token to send this account's requests as, from
/// whichever `EOAuth2Service` claims `source`.
///
/// This is where the refresh happens: EDS looks the account's refresh token up
/// in libsecret and exchanges it for an access token inside this call, so what
/// comes back is good now, and a failure is either "nobody has consented to
/// this account yet" or "the exchange did not work". Both are
/// [`ConnectError::OAuth2`], which asks Evolution to authenticate — see that
/// variant on why it never asks it to *discard* what it has.
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
        // message borrows a string the struct owns, and freeing it afterwards
        // is what the out-parameter contract asks for.
        let message = unsafe {
            let message = if error.is_null() {
                None
            } else {
                read_string((*error).message)
            };
            if !error.is_null() {
                g_error_free(error);
            }
            message
        };
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
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(ConnectError::OAuth2(translate(
            // TRANSLATORS: shown when EDS handed back an OAuth 2.0 access
            // token that was empty rather than absent.
            c"the OAuth 2.0 service returned an empty access token",
        ))),
    }
}
