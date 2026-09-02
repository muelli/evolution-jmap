// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `EOAuth2Service` implementation itself — the vtable [`oauth2`] built
//! the storage for.
//!
//! ## What is filled, and what is not
//!
//! Read against the installed EDS's own `e-oauth2-service.c`
//! (`gitlab.gnome.org/GNOME/evolution-data-server`, tag 3.52.3) rather than
//! against the header alone, which carries no `(transfer …)` or behavioural
//! annotation for any of this interface's vfuncs. That reading found each
//! wrapper function falls into one of three shapes, and which shape a vfunc
//! is in decides whether [`Vtable`] fills it:
//!
//! - **No default at all.** `get_name`, `get_display_name`, `get_client_id`
//!   and `get_authentication_uri` are `NULL` in the interface's own
//!   `default_init` — `eds-sys/tests/oauth2.rs`'s
//!   `the_slots_eds_leaves_empty_are_the_ones_a_service_must_fill` already
//!   pins this. Left unfilled, the wrapper's `g_return_val_if_fail` answers
//!   NULL and the service cannot authenticate anyone. Filled here.
//! - **A default that exists but only runs unfilled.** `get_refresh_uri`,
//!   `get_redirect_uri` and `get_client_secret` do have a real default (an
//!   OOB URN, `NULL`, `NULL` respectively) and the wrapper calls whatever is
//!   in the vtable unconditionally — no guard, no "try mine first". Ours
//!   varies per account (a deployment's own token endpoint, the redirect URI
//!   this client registered, and a secret only some servers issue), so the
//!   generic default is wrong for us and all three are filled here, each a
//!   direct read of [`oauth2`]'s storage.
//! - **A default that runs first, always, whether or not it is also
//!   filled.** `can_process`, `guess_can_process`,
//!   `prepare_authentication_uri_query`, `prepare_get_token_form` and
//!   `prepare_refresh_token_form` each call their own `eos_default_*` body
//!   *unconditionally*, and only call a filled slot afterwards — and only if
//!   that slot is not literally the same function pointer as the default,
//!   which is what stops the default recursing into itself for a type that
//!   never overrode the slot at all. So a service with nothing to add beyond
//!   what the RFC 6749 default already does — build the query and both token
//!   forms from `get_client_id`/`get_client_secret`/`get_redirect_uri`,
//!   accept or decline a source by its `[Authentication] method`, decline
//!   every hostname guess — does not need to fill these at all. `can_process`
//!   and `guess_can_process` are exactly that case and stay unfilled: matching
//!   on `[Authentication] method` is all this service wants, and no hostname
//!   pattern is worth guessing (every deployment is a different server).
//!
//!   The other three of the five *are* filled, because live deployments turned
//!   out to want parameters the RFC 6749 default knows nothing about: a `scope`
//!   (`error=invalid_scope`), an RFC 8707 `resource` (`error=invalid_target`)
//!   and RFC 7636 PKCE (`error=invalid_request`), all three observed against
//!   Fastmail on 2026-08-23. Filling them is additive only — see the next
//!   section for the rule that makes it safe, and
//!   `e-oauth2-service-google.c`/`-outlook.c` for EDS's own services keeping
//!   the same rule.
//!
//! `extract_authorization_code` and `extract_error_message` are the fourth
//! and only remaining pair: no anti-recursion guard, a real default
//! (`e_oauth2_service_util_extract_from_uri` against the navigated-to page's
//! own URI), and nothing account-specific to add. Left unfilled, for the
//! same reason as the five above.
//!
//! So seven vfuncs are filled, all of them either a `'static` constant or a
//! borrow of [`oauth2`]'s own storage — none compute a string that would need
//! freeing, which is the condition the previous session's storage work
//! existed to reach. Three more — the `prepare_*` trio — are filled as
//! *additive hooks*, per the next section.
//!
//! ## An additive hook, not a chain link
//!
//! For the third shape above, the slot a service fills is **not** a chain
//! link and must not call EDS's default itself. `e_oauth2_service_default_init`
//! (3.52.3) installs `eos_default_prepare_authentication_uri_query` and its two
//! siblings into the interface vtable, so the slot a subclass's
//! `interface_init` receives is already non-NULL — but each public wrapper
//! (`e-oauth2-service.c:648`, `:822`, `:895`) calls its `eos_default_*` body
//! *directly and unconditionally* before dispatching to the vtable, and skips
//! the vtable only when it still literally holds that same default. A filled
//! slot that also invoked the saved default would therefore run it twice.
//!
//! EDS's own services are the convention, not merely the reading of the
//! wrapper: `eos_google_prepare_authentication_uri_query` and
//! `eos_outlook_prepare_authentication_uri_query`/`_prepare_refresh_token_form`
//! each set only their own keys, and neither saves nor invokes the default.
//! This crate does the same. The double call it used to make was benign —
//! `e_oauth2_service_util_set_to_form` is insert-or-remove, so repeating it is
//! idempotent, and it is invisible through the wrapper for exactly that
//! reason — but it doubled every `get_client_id`/`get_client_secret`/
//! `get_redirect_uri` dispatch and would silently corrupt any future default
//! that appended rather than replaced. `tests/oauth2_service.rs` pins the rule
//! by dispatching the slots directly.
//!
//! ## What this crate does not do
//!
//! Set `[Authentication] method` to [`NAME`] itself — that is
//! [`backend::insert_entries`](crate::backend)'s job, the authentication
//! combo an account's setup writes from, and
//! [`config_lookup`](crate::config_lookup)'s successful-discovery result.
//! Both already exist, so an account reaches this service through either
//! path today, not only through a hand-edited `.source` keyfile.
//!
//! Registering the type is no longer on that list: both `e_module_load` entry
//! points call `register_dynamic::<Service>()`, so an `EOAuth2Services` built
//! in a process that loaded either module finds it — which is the condition
//! [`jmap_backend_core::oauth2`] needs for a source naming [`NAME`] to be
//! recognised as OAuth 2.0 and for its access token to be fetchable at all.
//!
//! Tested here the way `eds-sys/tests/oauth2.rs` tests the raw ABI: a
//! throwaway instance, dispatched through EDS's own `e_oauth2_service_*()`
//! wrappers.

use std::collections::HashMap;
use std::ffi::{CStr, c_char};
use std::ptr;
use std::sync::{Mutex, OnceLock};

use eds_sys::{
    EOAuth2Service, EOAuth2ServiceBase, EOAuth2ServiceBaseClass, EOAuth2ServiceInterface, ESource,
    e_oauth2_service_base_get_type, e_oauth2_service_get_type, e_source_get_uid,
};
use glib_sys::{GHashTable, GType, g_hash_table_replace, g_strdup};
use jmap_backend_core::i18n::{self, N_};
use jmap_backend_core::subclass::{InterfaceDecl, InterfaceImpl, ObjectSubclass};
use jmap_backend_core::trampoline::guard;

use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::marshal::read_string;
use jmap_client::oauth::PkceVerifier;

use crate::oauth2;

/// `e_oauth2_service_get_name`'s answer — not user-visible, so not
/// translated: it is the value the setup UI has to write as
/// `[Authentication] method` for `can_process`'s default matching (see the
/// module docs) to ever say yes.
///
/// Re-exported rather than spelled out: `jmap-mail`'s named `CamelSasl` has to
/// give the same string as its `authproto` and cannot see this crate, so the
/// literal lives in the crate both can reach. See
/// [`jmap_backend_core::oauth2::OAUTH2_SERVICE_NAME`] for the three places
/// that have to agree.
pub const NAME: &CStr = jmap_backend_core::oauth2::OAUTH2_SERVICE_NAME;

/// The instance struct: nothing but [`EOAuth2ServiceBase`]'s own state. No
/// per-instance storage of our own — everything this service answers is
/// either `'static` or reached through the `source` argument every
/// per-account vfunc is handed.
#[repr(C)]
pub struct Service {
    parent: EOAuth2ServiceBase,
}

/// The class struct: nothing but [`EOAuth2ServiceBaseClass`]'s own state.
#[repr(C)]
pub struct ServiceClass {
    parent_class: EOAuth2ServiceBaseClass,
}

// SAFETY: both structs are #[repr(C)] and lead with EOAuth2ServiceBase's own
// instance/class structs; EOAuth2ServiceBase is `struct { EExtension parent; }`
// and EExtension derives from GObject (checked in eds-sys/tests/oauth2.rs).
unsafe impl ObjectSubclass for Service {
    const NAME: &'static CStr = c"JmapOAuth2Service";
    type Instance = Service;
    type Class = ServiceClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_oauth2_service_base_get_type() }
    }

    fn interfaces() -> Vec<InterfaceDecl> {
        vec![InterfaceDecl::filled_by::<Vtable>()]
    }
}

/// The filling of [`Service`]'s copy of `EOAuth2ServiceInterface` — see the
/// module docs for which slots and why.
pub struct Vtable;

// SAFETY: `EOAuth2ServiceInterface` is bindgen's `#[repr(C)]` binding of the
// interface struct `e_oauth2_service_get_type` names, and it leads with
// `GTypeInterface` — eds-sys's tests/oauth2.rs pins the interface's shape.
unsafe impl InterfaceImpl for Vtable {
    type Vtable = EOAuth2ServiceInterface;

    fn gtype() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_oauth2_service_get_type() }
    }

    unsafe fn interface_init(vtable: *mut Self::Vtable) {
        // SAFETY: the contract of `InterfaceImpl::interface_init` — this is
        // our own copy of the vtable, and nothing else can reach it yet.
        let vtable = unsafe { &mut *vtable };
        vtable.get_name = Some(get_name);
        vtable.get_display_name = Some(get_display_name);
        vtable.get_client_id = Some(get_client_id);
        vtable.get_client_secret = Some(get_client_secret);
        vtable.get_authentication_uri = Some(get_authentication_uri);
        vtable.get_refresh_uri = Some(get_refresh_uri);
        vtable.get_redirect_uri = Some(get_redirect_uri);
        // These three slots arrive pre-filled with EDS's own defaults (3.52.3
        // `e_oauth2_service_default_init`), which build the standard RFC 6749
        // query and both token forms. Displacing them loses nothing: each
        // public wrapper runs its default *itself*, unconditionally, before
        // dispatching here — see the "additive hook, not a chain link"
        // section of the module docs. So these override bodies only add.
        vtable.prepare_authentication_uri_query = Some(prepare_authentication_uri_query);
        vtable.prepare_get_token_form = Some(prepare_get_token_form);
        vtable.prepare_refresh_token_form = Some(prepare_refresh_token_form);
    }
}

/// Adds the RFC 6749 §3.3 `scope` this client registered for (stored on the
/// source by the discovery worker) to the authorization request, after EDS's
/// default has built the standard query.
///
/// Sent explicitly rather than left to the registered-default fallback:
/// whether an omitted `scope` falls back to the registration's is
/// server-discretionary, and Fastmail answered `error=invalid_scope` to
/// exactly that omission (observed live 2026-08-23). A source with no stored
/// scope — a deployment that advertises none — keeps the query untouched.
unsafe extern "C" fn prepare_authentication_uri_query(
    _service: *mut EOAuth2Service,
    source: *mut ESource,
    uri_query: *mut GHashTable,
) {
    guard(
        "JmapOAuth2Service::prepare_authentication_uri_query",
        (),
        || unsafe {
            let uid = read_string(e_source_get_uid(source));
            let scope = oauth2::scope(source);
            let has_scope = !scope.is_null() && *scope != 0;
            if has_scope {
                // The table frees both halves (EDS builds it with g_free
                // destroyers), so both are handed over as fresh copies.
                g_hash_table_replace(
                    uri_query,
                    g_strdup(c"scope".as_ptr()).cast(),
                    g_strdup(scope).cast(),
                );
            }
            add_resource(source, uri_query);
            // RFC 7636 PKCE, which EDS 3.52 does not know at all (its
            // libedataserver contains no `code_challenge` — checked with
            // `strings`, not assumed) and which a provider may mandate for
            // public clients: Fastmail advertises only S256 and its consent
            // flow answers `error=invalid_request` to a challenge-less
            // request (observed live 2026-08-23, after scope and resource
            // were already right). The verifier is stashed per source UID
            // for `prepare_get_token_form` to redeem; a repeated
            // authorization attempt simply replaces it.
            //
            // All-or-nothing (F16 audit follow-up, `docs/AUDIT-FFI-20260828.md`
            // "F17"): a challenge is only ever added once its verifier is
            // stashed. A uid-less source could not be stashed against, and
            // sending a challenge with nobody able to redeem it is worse than
            // sending none — RFC 7636 §4.6 obliges the server to reject the
            // code exchange that follows, so this would previously turn into
            // a silent, unattributable authentication failure rather than the
            // well-defined "no PKCE" request this deployment already tolerates
            // (it is EDS's own default query without our addition).
            let verifier = if let Some(ref uid) = uid {
                let verifier = PkceVerifier::generate();
                pkce_verifiers()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(uid.clone(), verifier.secret().to_owned());
                g_hash_table_replace(
                    uri_query,
                    g_strdup(c"code_challenge".as_ptr()).cast(),
                    g_strdup(cstring_lossy(&verifier.challenge()).as_ptr()).cast(),
                );
                g_hash_table_replace(
                    uri_query,
                    g_strdup(c"code_challenge_method".as_ptr()).cast(),
                    g_strdup(c"S256".as_ptr()).cast(),
                );
                true
            } else {
                // Not reached through any path EDS's public API can build
                // today — `e_source_new*` always assigns a uid — but the
                // fallback is cheap insurance against a source that somehow
                // has none, and is exactly the case a plain `debug!` would
                // have hidden per the audit's own finding below.
                tracing::warn!(
                    "source has no uid; omitting the PKCE challenge rather than \
                     sending one whose verifier cannot be stashed and so can \
                     never be redeemed"
                );
                false
            };
            tracing::debug!(
                account_uid = ?uid,
                has_scope,
                has_pkce = verifier,
                "prepared OAuth 2.0 authentication uri query"
            );
        },
    );
}

/// The PKCE verifiers awaiting their token exchange, keyed by source UID —
/// in-memory only: a verifier is a per-flow secret with no business in the
/// on-disk keyfile, and both halves of the flow run in the same process (the
/// credentials prompter's).
fn pkce_verifiers() -> &'static Mutex<HashMap<String, String>> {
    static VERIFIERS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    VERIFIERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Adds the stored RFC 8707 `resource` indicator to `table` when the source
/// carries one; a source without one (a deployment the discovery probe could
/// not classify, or that predates resource indicators) leaves the request
/// exactly as EDS's default built it.
///
/// # Safety
///
/// `source` must be a valid `ESource` and `table` a live `GHashTable` with
/// `g_free` destroyers for both halves.
unsafe fn add_resource(source: *mut ESource, table: *mut GHashTable) {
    // SAFETY: the caller's contract.
    unsafe {
        let resource = oauth2::resource(source);
        if !resource.is_null() && *resource != 0 {
            g_hash_table_replace(
                table,
                g_strdup(c"resource".as_ptr()).cast(),
                g_strdup(resource).cast(),
            );
        }
    }
}

/// RFC 8707 §2.2: the resource is named when the code is redeemed, not only
/// when it is authorized — a deployment that requires it at one end requires
/// it at both (Fastmail's `error=invalid_target`, observed live 2026-08-23,
/// is the authorization half of that requirement).
unsafe extern "C" fn prepare_get_token_form(
    _service: *mut EOAuth2Service,
    source: *mut ESource,
    _authorization_code: *const c_char,
    form: *mut GHashTable,
) {
    guard("JmapOAuth2Service::prepare_get_token_form", (), || unsafe {
        let uid = read_string(e_source_get_uid(source));
        add_resource(source, form);
        // Redeem the PKCE verifier stashed when the authorization query was
        // built (RFC 7636 §4.5). Taken, not copied: a verifier is single-use.
        let has_pkce = if let Some(ref uid) = uid
            && let Some(verifier) = pkce_verifiers()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(uid)
        {
            g_hash_table_replace(
                form,
                g_strdup(c"code_verifier".as_ptr()).cast(),
                g_strdup(cstring_lossy(&verifier).as_ptr()).cast(),
            );
            true
        } else {
            false
        };
        if !has_pkce {
            // This is the failure mode that turns into the operator's
            // repeating consent window (`docs/AUDIT-FFI-20260828.md` "F17"):
            // every code exchange this service prepares follows a
            // `prepare_authentication_uri_query` call that stashed a
            // verifier, so its absence here means either that request never
            // ran in this process, or the challenge it sent (before the
            // all-or-nothing fix above) could never be redeemed — either way
            // the exchange that follows is expected to fail, and `warn!`
            // (not `debug!`) is what makes that attributable from the
            // journal alone, per items 20(3)/22(3).
            tracing::warn!(
                account_uid = ?uid,
                "no stashed PKCE verifier for this authorization code exchange; \
                 if a challenge was sent, the server is expected to reject it"
            );
        }
        tracing::debug!(
            account_uid = ?uid,
            has_pkce,
            "prepared OAuth 2.0 get token form"
        );
    });
}

/// As [`prepare_get_token_form`], for the refresh grant.
unsafe extern "C" fn prepare_refresh_token_form(
    _service: *mut EOAuth2Service,
    source: *mut ESource,
    refresh_token: *const c_char,
    form: *mut GHashTable,
) {
    guard(
        "JmapOAuth2Service::prepare_refresh_token_form",
        (),
        || unsafe {
            let uid = read_string(e_source_get_uid(source));
            add_resource(source, form);
            let has_refresh_token = !refresh_token.is_null() && *refresh_token != 0;
            tracing::debug!(
                account_uid = ?uid,
                has_refresh_token,
                "prepared OAuth 2.0 refresh token form"
            );
        },
    );
}

unsafe extern "C" fn get_name(_service: *mut EOAuth2Service) -> *const c_char {
    guard("JmapOAuth2Service::get_name", ptr::null(), || NAME.as_ptr())
}

/// The one user-visible string here — the account editor's label for this
/// service — so it goes through [`i18n::translate_static`] rather than
/// answering [`NAME`] itself. Marked `N_` for `xgettext` since the lookup is
/// this function's own, not a `CamelProvider`'s `dgettext` call on a
/// `'static` constant the way `provider.rs`'s strings are.
unsafe extern "C" fn get_display_name(_service: *mut EOAuth2Service) -> *const c_char {
    guard("JmapOAuth2Service::get_display_name", ptr::null(), || {
        i18n::translate_static(N_(c"JMAP"))
    })
}

unsafe extern "C" fn get_client_id(
    _service: *mut EOAuth2Service,
    source: *mut ESource,
) -> *const c_char {
    guard("JmapOAuth2Service::get_client_id", ptr::null(), || unsafe {
        oauth2::client_id(source)
    })
}

unsafe extern "C" fn get_client_secret(
    _service: *mut EOAuth2Service,
    source: *mut ESource,
) -> *const c_char {
    guard(
        "JmapOAuth2Service::get_client_secret",
        ptr::null(),
        || unsafe { oauth2::client_secret(source) },
    )
}

unsafe extern "C" fn get_authentication_uri(
    _service: *mut EOAuth2Service,
    source: *mut ESource,
) -> *const c_char {
    guard(
        "JmapOAuth2Service::get_authentication_uri",
        ptr::null(),
        || unsafe { oauth2::authorization_endpoint(source) },
    )
}

unsafe extern "C" fn get_refresh_uri(
    _service: *mut EOAuth2Service,
    source: *mut ESource,
) -> *const c_char {
    guard(
        "JmapOAuth2Service::get_refresh_uri",
        ptr::null(),
        || unsafe { oauth2::token_endpoint(source) },
    )
}

unsafe extern "C" fn get_redirect_uri(
    _service: *mut EOAuth2Service,
    source: *mut ESource,
) -> *const c_char {
    guard(
        "JmapOAuth2Service::get_redirect_uri",
        ptr::null(),
        || unsafe { oauth2::redirect_uri(source) },
    )
}
