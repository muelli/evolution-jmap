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
//!   every hostname guess — does not need to fill these at all. **JMAP has
//!   nothing to add**: no OAuth scope parameter (RFC 8620 grants everything
//!   the token's account can see), no non-standard token endpoint quirks, and
//!   no hostname pattern worth guessing (every deployment is a different
//!   server). `e-oauth2-service-google.c` is the confirmation this is the
//!   real convention and not a theoretical reading of the wrapper: Google's
//!   own service overrides none of these five either, for exactly this
//!   reason — its `prepare_authentication_uri_query` override only *adds* a
//!   scope, on top of what the default already filled in.
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
//! existed to reach.
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
pub const NAME: &CStr = c"JMAP";

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
        // The interface's own slot arrives pre-filled with EDS's default,
        // which builds the standard RFC 6749 query but knows nothing of
        // scope — concrete services add their own (EDS's Google and Outlook
        // implementations do exactly this). Keep the default reachable and
        // chain to it, then add ours.
        if let Some(default) = vtable.prepare_authentication_uri_query {
            let _ = DEFAULT_PREPARE_AUTHENTICATION_URI_QUERY.set(default);
        }
        vtable.prepare_authentication_uri_query = Some(prepare_authentication_uri_query);
        // RFC 8707 names the resource on the token grants too, so both form
        // builders get the same chain-and-add treatment as the query above.
        if let Some(default) = vtable.prepare_get_token_form {
            let _ = DEFAULT_PREPARE_GET_TOKEN_FORM.set(default);
        }
        vtable.prepare_get_token_form = Some(prepare_get_token_form);
        if let Some(default) = vtable.prepare_refresh_token_form {
            let _ = DEFAULT_PREPARE_REFRESH_TOKEN_FORM.set(default);
        }
        vtable.prepare_refresh_token_form = Some(prepare_refresh_token_form);
    }
}

/// EDS's own `prepare_authentication_uri_query`, saved by `interface_init`
/// before being displaced, so the override below can chain to it.
static DEFAULT_PREPARE_AUTHENTICATION_URI_QUERY: OnceLock<
    unsafe extern "C" fn(*mut EOAuth2Service, *mut ESource, *mut GHashTable),
> = OnceLock::new();

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
    service: *mut EOAuth2Service,
    source: *mut ESource,
    uri_query: *mut GHashTable,
) {
    guard(
        "JmapOAuth2Service::prepare_authentication_uri_query",
        (),
        || unsafe {
            if let Some(default) = DEFAULT_PREPARE_AUTHENTICATION_URI_QUERY.get() {
                default(service, source, uri_query);
            }
            let scope = oauth2::scope(source);
            if !scope.is_null() && *scope != 0 {
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
            let verifier = PkceVerifier::generate();
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
            if let Some(uid) = read_string(e_source_get_uid(source)) {
                pkce_verifiers()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(uid, verifier.secret().to_owned());
            }
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

/// EDS's own `prepare_get_token_form`, saved by `interface_init` before being
/// displaced, so the override below can chain to it.
static DEFAULT_PREPARE_GET_TOKEN_FORM: OnceLock<
    unsafe extern "C" fn(*mut EOAuth2Service, *mut ESource, *const c_char, *mut GHashTable),
> = OnceLock::new();

/// EDS's own `prepare_refresh_token_form`, saved the same way.
static DEFAULT_PREPARE_REFRESH_TOKEN_FORM: OnceLock<
    unsafe extern "C" fn(*mut EOAuth2Service, *mut ESource, *const c_char, *mut GHashTable),
> = OnceLock::new();

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
    service: *mut EOAuth2Service,
    source: *mut ESource,
    authorization_code: *const c_char,
    form: *mut GHashTable,
) {
    guard("JmapOAuth2Service::prepare_get_token_form", (), || unsafe {
        if let Some(default) = DEFAULT_PREPARE_GET_TOKEN_FORM.get() {
            default(service, source, authorization_code, form);
        }
        add_resource(source, form);
        // Redeem the PKCE verifier stashed when the authorization query was
        // built (RFC 7636 §4.5). Taken, not copied: a verifier is single-use.
        if let Some(uid) = read_string(e_source_get_uid(source))
            && let Some(verifier) = pkce_verifiers()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&uid)
        {
            g_hash_table_replace(
                form,
                g_strdup(c"code_verifier".as_ptr()).cast(),
                g_strdup(cstring_lossy(&verifier).as_ptr()).cast(),
            );
        }
    });
}

/// As [`prepare_get_token_form`], for the refresh grant.
unsafe extern "C" fn prepare_refresh_token_form(
    service: *mut EOAuth2Service,
    source: *mut ESource,
    refresh_token: *const c_char,
    form: *mut GHashTable,
) {
    guard(
        "JmapOAuth2Service::prepare_refresh_token_form",
        (),
        || unsafe {
            if let Some(default) = DEFAULT_PREPARE_REFRESH_TOKEN_FORM.get() {
                default(service, source, refresh_token, form);
            }
            add_resource(source, form);
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
