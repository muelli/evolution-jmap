// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `EOAuth2Service` implementation: the vtable that uses the storage in
//! [`oauth2`].
//!
//! ## Which slots are filled
//!
//! This comes from the installed EDS's own `e-oauth2-service.c`
//! (`gitlab.gnome.org/GNOME/evolution-data-server`, tag 3.52.3), not only from
//! the header. The header has no `(transfer …)` or behaviour annotation for
//! the vfuncs of this interface. Each wrapper function has one of three
//! shapes, and the shape of a vfunc decides if [`Vtable`] fills it:
//!
//! - **No default.** `get_name`, `get_display_name`, `get_client_id` and
//!   `get_authentication_uri` are `NULL` in the interface's own
//!   `default_init`. `eds-sys/tests/oauth2.rs`'s
//!   `the_slots_eds_leaves_empty_are_the_ones_a_service_must_fill` makes sure
//!   of this. If a slot is empty, the wrapper's `g_return_val_if_fail` returns
//!   NULL and the service cannot authenticate. This module fills them.
//! - **A default that runs only if the slot is empty.** `get_refresh_uri`,
//!   `get_redirect_uri` and `get_client_secret` have a real default (an OOB
//!   URN, `NULL` and `NULL`). The wrapper always calls the vtable entry, with
//!   no guard and no "try mine first". The values of this service change for
//!   each account (the token endpoint of a deployment, the registered redirect
//!   URI, and a secret that only some servers issue). Thus the generic default
//!   is wrong, and this module fills all three. Each reads [`oauth2`]'s
//!   storage directly.
//! - **A default that always runs first, also if the slot is filled.**
//!   `can_process`, `guess_can_process`, `prepare_authentication_uri_query`,
//!   `prepare_get_token_form` and `prepare_refresh_token_form` each call their
//!   own `eos_default_*` body *unconditionally*. Then they call a filled slot,
//!   but only if that slot is not the same function pointer as the default.
//!   That check stops the default from recursion into itself for a type that
//!   did not override the slot. The RFC 6749 default builds the query and both
//!   token forms from `get_client_id`/`get_client_secret`/`get_redirect_uri`,
//!   accepts or declines a source by its `[Authentication] method`, and
//!   declines all hostname guesses. A service that adds nothing to this does
//!   not need to fill these slots. `can_process` and `guess_can_process` are
//!   that case and stay empty: a match on `[Authentication] method` is
//!   sufficient, and no hostname pattern is useful (each deployment is a
//!   different server).
//!
//!   This module fills the other three of the five. Live deployments need
//!   parameters that the RFC 6749 default does not know: a `scope`
//!   (`error=invalid_scope`), an RFC 8707 `resource` (`error=invalid_target`)
//!   and RFC 7636 PKCE (`error=invalid_request`). All three errors came from
//!   Fastmail on 2026-08-23. These slots only add: the next section gives the
//!   rule that makes this safe. `e-oauth2-service-google.c`/`-outlook.c` show
//!   EDS's own services with the same rule.
//!
//! `extract_authorization_code` and `extract_error_message` are the fourth
//! and last pair. They have no anti-recursion guard, a real default
//! (`e_oauth2_service_util_extract_from_uri` on the URI of the page that the
//! browser opened), and nothing account-specific to add. Thus they stay
//! empty, for the same reason as `can_process` and `guess_can_process`.
//!
//! Thus seven vfuncs are filled. Each returns a `'static` constant or a borrow
//! of [`oauth2`]'s own storage. None computes a string that must be freed.
//! [`oauth2`]'s module docs say that this ownership shape is why that storage
//! exists. The three `prepare_*` slots are also filled, as *additive hooks*
//! (see the next section).
//!
//! ## An additive hook, not a chain link
//!
//! For the third shape above, the filled slot is **not** a chain link and
//! must not call EDS's default itself. `e_oauth2_service_default_init`
//! (3.52.3) installs `eos_default_prepare_authentication_uri_query` and the
//! two related defaults into the interface vtable (`e-oauth2-service.c:320`,
//! `:322`, `:324`). Thus the slot that a subclass's `interface_init` receives
//! is not NULL. But each public wrapper (`:648`, `:822`, `:895`) calls its
//! `eos_default_*` body *directly and unconditionally* before it dispatches to
//! the vtable. It skips the vtable only if the vtable still holds that same
//! default (`:654`, `:828`, `:901`). Thus a filled slot that also calls the
//! saved default runs the default two times.
//!
//! EDS's own services follow this convention, so it is not only one reading
//! of the wrapper. `eos_google_prepare_authentication_uri_query` and
//! `eos_outlook_prepare_authentication_uri_query`/`_prepare_refresh_token_form`
//! each set only their own keys, and they do not save or call the default.
//! This crate does the same. The double call that it made before caused no
//! damage: `e_oauth2_service_util_set_to_form` inserts or removes, so a
//! repeat is idempotent, and the wrapper thus shows no difference. But it
//! doubled each `get_client_id`/`get_client_secret`/`get_redirect_uri`
//! dispatch. Also, it would silently corrupt a future default that appends
//! and does not replace. `tests/oauth2_service.rs` makes sure of the rule: it
//! dispatches the slots directly.
//!
//! ## What this module does not do
//!
//! It does not set `[Authentication] method` to [`NAME`].
//! [`backend::insert_entries`](crate::backend) does that (it fills the
//! authentication combo that the account configuration writes from), and so
//! does the successful-discovery result of
//! [`config_lookup`](crate::config_lookup). Thus an account gets to this
//! service through either path, not only through a manually edited
//! `.source` keyfile.
//!
//! Type registration is not on that list now. Each `e_module_load` entry
//! point calls [`register`]. Thus an `EOAuth2Services` in a process that
//! loaded such a module finds the type. [`jmap_backend_core::oauth2`] needs
//! this before it can recognise a source with [`NAME`] as OAuth 2.0 and get
//! its access token.
//!
//! The tests here use the same method as `eds-sys/tests/oauth2.rs` for the raw
//! ABI: a temporary instance, dispatched through EDS's own
//! `e_oauth2_service_*()` wrappers.

use std::collections::HashMap;
use std::ffi::{CStr, c_char};
use std::ptr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use eds_sys::{
    EOAuth2Service, EOAuth2ServiceBase, EOAuth2ServiceBaseClass, EOAuth2ServiceInterface, ESource,
    e_oauth2_service_base_get_type, e_oauth2_service_get_type, e_source_get_uid,
};
use glib_sys::{GHashTable, GType, g_hash_table_replace, g_strdup, g_strndup};
use jmap_backend_core::i18n::{self, N_};
use jmap_backend_core::subclass::{InterfaceDecl, InterfaceImpl, ObjectSubclass};
use jmap_backend_core::trampoline::guard;

use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::marshal::read_string;
use jmap_client::oauth::PkceVerifier;
use zeroize::Zeroizing;

use crate::oauth2;

/// The return value of `e_oauth2_service_get_name`. The user does not see it,
/// so it is not translated. The configuration UI must write this value as
/// `[Authentication] method`, or the default match of `can_process` (see the
/// module docs) never accepts the source.
///
/// This is a re-export, not a new literal. `jmap-mail`'s named `CamelSasl` must
/// give the same string as its `authproto`, and it cannot see this crate. Thus
/// the literal is in a crate that both can use. See
/// [`jmap_backend_core::oauth2::OAUTH2_SERVICE_NAME`] for the three places
/// that must agree.
pub const NAME: &CStr = jmap_backend_core::oauth2::OAUTH2_SERVICE_NAME;

/// Registers [`Service`] with `type_module`. If the running EDS has its own
/// `EOAuth2ServiceDynamic`, it registers nothing and that service controls the
/// account.
///
/// All modules call this function, not `register_dynamic::<Service>`
/// directly. Thus the decision to stand down is made one time, here, not in
/// each module with a `#[cfg]` and a `build.rs` that emits it again. The
/// decision reads
/// [`EDS_HAS_DYNAMIC_OAUTH2`](jmap_backend_core::oauth2::EDS_HAS_DYNAMIC_OAUTH2),
/// a `const bool`, so the compiler removes the unused branch on both build types.
///
/// The stand-down is necessary. This service and upstream's would both accept
/// `can_process` for an account whose method names either one.
/// `e_oauth2_services_find` returns the first match in a list, and this crate
/// does not control the order of that list. Also, an unused registration
/// still puts an inactive "JMAP" entry in Evolution's *Authentication type*
/// combo, because `e_auth_combo_box_update_available` builds that list from
/// the registered services.
///
/// # Safety
///
/// `type_module` must be the `GTypeModule *` the caller's `e_module_load` was
/// passed, alive for the duration of the call.
pub unsafe fn register(type_module: *mut gobject_sys::GTypeModule) {
    if jmap_backend_core::oauth2::EDS_HAS_DYNAMIC_OAUTH2 {
        // No migration, by design. An account configured with an older EDS
        // keeps the method "JMAP" and its `[JMAP OAuth2]` group. A build that
        // takes this branch reads neither, so the account stops and must be
        // configured again. Do not add migration code here. A migration
        // cannot move the stored tokens: the keyring key is
        // `OAuth2::<service name>[user][credential-store-id]`, so the new
        // service cannot see the old tokens.
        return;
    }
    // SAFETY: a live `GTypeModule` by this function's contract.
    unsafe { jmap_backend_core::subclass::register_dynamic::<Service>(type_module) };
}

/// The instance struct: only [`EOAuth2ServiceBase`]'s own state. This service
/// has no per-instance storage. All its return values are `'static` or come
/// through the `source` argument of each per-account vfunc.
#[repr(C)]
pub struct Service {
    parent: EOAuth2ServiceBase,
}

/// The class struct: only [`EOAuth2ServiceBaseClass`]'s own state.
#[repr(C)]
pub struct ServiceClass {
    parent_class: EOAuth2ServiceBaseClass,
}

// SAFETY: both structs are #[repr(C)] and start with EOAuth2ServiceBase's own
// instance/class structs. EOAuth2ServiceBase is `struct { EExtension parent; }`
// and EExtension derives from GObject (eds-sys/tests/oauth2.rs checks this).
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

/// Fills [`Service`]'s copy of `EOAuth2ServiceInterface`. The module docs tell
/// which slots and why.
pub struct Vtable;

// SAFETY: `EOAuth2ServiceInterface` is bindgen's `#[repr(C)]` binding of the
// interface struct of `e_oauth2_service_get_type`, and it starts with
// `GTypeInterface`. eds-sys's tests/oauth2.rs checks the shape of the interface.
unsafe impl InterfaceImpl for Vtable {
    type Vtable = EOAuth2ServiceInterface;

    fn gtype() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_oauth2_service_get_type() }
    }

    unsafe fn interface_init(vtable: *mut Self::Vtable) {
        // SAFETY: the contract of `InterfaceImpl::interface_init` applies. This
        // is the copy of the vtable for this type, and nothing else can reach it yet.
        let vtable = unsafe { &mut *vtable };
        vtable.get_name = Some(get_name);
        vtable.get_display_name = Some(get_display_name);
        vtable.get_client_id = Some(get_client_id);
        vtable.get_client_secret = Some(get_client_secret);
        vtable.get_authentication_uri = Some(get_authentication_uri);
        vtable.get_refresh_uri = Some(get_refresh_uri);
        vtable.get_redirect_uri = Some(get_redirect_uri);
        // These three slots already hold EDS's own defaults (3.52.3
        // `e_oauth2_service_default_init`), which build the standard RFC 6749
        // query and both token forms. Their replacement removes nothing: each
        // public wrapper runs its default *itself*, unconditionally, before it
        // dispatches here (see "An additive hook, not a chain link" in the
        // module docs). Thus these bodies only add.
        vtable.prepare_authentication_uri_query = Some(prepare_authentication_uri_query);
        vtable.prepare_get_token_form = Some(prepare_get_token_form);
        vtable.prepare_refresh_token_form = Some(prepare_refresh_token_form);
    }
}

/// Adds the registered RFC 6749 §3.3 `scope` (which the discovery worker stores
/// on the source) to the authorization request, after EDS's default builds the
/// standard query. Also adds the `resource` and the PKCE challenge.
///
/// The scope is sent explicitly. The server decides if an omitted `scope` gets
/// the registered scope, and Fastmail returned `error=invalid_scope` for that
/// omission (seen live 2026-08-23). A source with no stored scope (a
/// deployment that advertises none) gets no `scope`.
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
            let has_scope = read_string(scope).is_some();
            if has_scope {
                // The table frees both key and value (EDS builds it with g_free
                // destroyers), so both are new copies.
                g_hash_table_replace(
                    uri_query,
                    g_strdup(c"scope".as_ptr()).cast(),
                    g_strdup(scope).cast(),
                );
            }
            add_resource(source, uri_query);
            // RFC 7636 PKCE. EDS 3.52 does not know it (its libedataserver
            // contains no `code_challenge`: checked with `strings`, not
            // assumed). A provider can require it for public clients: Fastmail
            // advertises only S256, and its consent flow returns
            // `error=invalid_request` for a request without a challenge (seen
            // live 2026-08-23, when scope and resource were already correct).
            // The verifier is stashed per source UID, and
            // `prepare_get_token_form` redeems it. A repeated authorization
            // replaces it (see [`Verifiers`]).
            //
            // All or nothing: a challenge is added only after its verifier is
            // stashed. A source without a uid has no stash key. A challenge
            // that nobody can redeem is worse than no challenge: RFC 7636
            // section 4.6 requires the server to reject the code exchange
            // that follows. The result would be a silent authentication failure
            // without a known cause. Without a challenge, the result is the
            // well-defined "no PKCE" request (EDS's own default query without
            // this addition), which all deployments that this project targets
            // accept.
            let has_pkce = if let Some(ref uid) = uid {
                let verifier = PkceVerifier::generate();
                pkce_verifiers()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stash(
                        uid.clone(),
                        Zeroizing::new(verifier.secret().to_owned()),
                        Instant::now(),
                    );
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
                // EDS's public API cannot make a source that gets here:
                // `e_source_new*` assigns a uid if the caller gives none. This
                // fallback is a cheap protection against a source without a
                // uid. It warns and is not silent, because a plain debug! hid
                // the failure that this branch prevents.
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
                has_pkce,
                "prepared OAuth 2.0 authentication uri query"
            );
        },
    );
}

/// The PKCE verifiers that wait for their token exchange, keyed by source UID.
/// They are in memory only: a verifier is a per-flow secret that must not be
/// in the on-disk keyfile. Both parts of the flow run in the same process (the
/// process of the credentials prompter).
fn pkce_verifiers() -> &'static Mutex<Verifiers> {
    static VERIFIERS: OnceLock<Mutex<Verifiers>> = OnceLock::new();
    VERIFIERS.get_or_init(|| Mutex::new(Verifiers::default()))
}

/// The maximum time that a stashed verifier waits for its token exchange. EDS
/// does not tell an `EOAuth2Service` when the user closes the consent window.
/// Without this limit, the verifier of an abandoned flow stays for the life of
/// the process. The limit must include the user's time on the consent page
/// and the lifetime of the code, which RFC 6749 section 4.1.2 recommends to be
/// ten minutes or less. A verifier older than this is for a consent that
/// nobody will complete. If the user completes it, the exchange has no
/// verifier, the server refuses it, and EDS asks for consent again.
const VERIFIER_LIFETIME: Duration = Duration::from_secs(30 * 60);

/// One verifier that waits for its token exchange.
struct Pending {
    /// Wiped at each drop: when replaced, redeemed or evicted.
    secret: Zeroizing<String>,
    stashed: Instant,
}

/// The PKCE verifiers that wait for their token exchange, keyed by source UID.
///
/// Each verifier is a [`Zeroizing`] secret. Thus its bytes are overwritten
/// when a repeated authorization replaces it, when the caller drops it after
/// [`redeem`](Self::redeem), and when it is evicted. Eviction runs on each
/// stash and redeem, for all accounts, and drops all entries older than
/// [`VERIFIER_LIFETIME`]. A move of a `Pending` (on insert, or when the map
/// grows) moves the `String`'s pointer, not the heap bytes. Thus no unwiped
/// copy stays.
///
/// Not covered: `jmap_client::oauth::PkceVerifier`, from which the generated
/// secret is copied, does not wipe its own copy on drop.
#[derive(Default)]
struct Verifiers(HashMap<String, Pending>);

impl Verifiers {
    fn stash(&mut self, uid: String, secret: Zeroizing<String>, now: Instant) {
        self.evict_expired(now);
        self.0.insert(
            uid,
            Pending {
                secret,
                stashed: now,
            },
        );
    }

    /// Removes and returns the verifier for `uid`, if one is stashed and its
    /// lifetime did not end. It is not copied: a verifier is single-use.
    fn redeem(&mut self, uid: &str, now: Instant) -> Option<Zeroizing<String>> {
        self.evict_expired(now);
        self.0.remove(uid).map(|pending| pending.secret)
    }

    fn evict_expired(&mut self, now: Instant) {
        self.0.retain(|_, pending| {
            now.saturating_duration_since(pending.stashed) < VERIFIER_LIFETIME
        });
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.len()
    }
}

/// Adds the stored RFC 8707 `resource` indicator to `table` if the source has
/// one. For a source without one (the discovery probe could not find it, or
/// the deployment is older than resource indicators), the request stays as
/// EDS's default built it.
///
/// # Safety
///
/// `source` must be a valid `ESource` and `table` a live `GHashTable` with
/// `g_free` destroyers for both halves.
unsafe fn add_resource(source: *mut ESource, table: *mut GHashTable) {
    // SAFETY: the caller's contract applies.
    unsafe {
        let resource = oauth2::resource(source);
        if read_string(resource).is_some() {
            g_hash_table_replace(
                table,
                g_strdup(c"resource".as_ptr()).cast(),
                g_strdup(resource).cast(),
            );
        }
    }
}

/// Adds the `resource` and the PKCE verifier to the token form. RFC 8707 §2.2:
/// the resource is named when the code is redeemed, not only at authorization.
/// A deployment that requires it at one step requires it at both (Fastmail's
/// `error=invalid_target`, seen live 2026-08-23, is the authorization part).
unsafe extern "C" fn prepare_get_token_form(
    _service: *mut EOAuth2Service,
    source: *mut ESource,
    _authorization_code: *const c_char,
    form: *mut GHashTable,
) {
    guard("JmapOAuth2Service::prepare_get_token_form", (), || unsafe {
        let uid = read_string(e_source_get_uid(source));
        add_resource(source, form);
        // Redeem the PKCE verifier from the authorization query (RFC 7636 §4.5).
        // It is removed, not copied: a verifier is single-use.
        let has_pkce = if let Some(ref uid) = uid
            && let Some(verifier) = pkce_verifiers()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .redeem(uid, Instant::now())
        {
            // Copied directly into the form's own allocation, with no
            // intermediate `CString` that is freed unwiped. EDS frees the
            // form's copy. A verifier is base64url, so it has no NUL that
            // stops `g_strndup`.
            g_hash_table_replace(
                form,
                g_strdup(c"code_verifier".as_ptr()).cast(),
                g_strndup(verifier.as_ptr().cast(), verifier.len()).cast(),
            );
            true
        } else {
            // A warning on purpose. If the authorization request had a
            // challenge, RFC 7636 section 4.6 requires the server to reject the
            // exchange of this form, and the user sees only a consent window
            // that opens again. At debug! level, logs usually do not capture
            // it, and it looks the same as the correct "this deployment did not
            // use PKCE" case. Thus the journal did not show the cause.
            tracing::warn!(
                account_uid = ?uid,
                "no stashed PKCE verifier for this account; the authorization \
                 request may have carried a challenge, in which case the server \
                 is expected to reject this code exchange"
            );
            false
        };
        tracing::debug!(
            account_uid = ?uid,
            has_pkce,
            "prepared OAuth 2.0 get token form"
        );
    });
}

/// Adds the `resource` to the refresh grant form, as [`prepare_get_token_form`]
/// does. A refresh needs no PKCE verifier.
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
            let has_refresh_token = read_string(refresh_token).is_some();
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

/// The only string here that the user sees: the account editor's label for
/// this service. Thus it goes through [`i18n::translate_static`] and does not
/// return [`NAME`]. It has the `N_` mark for `xgettext` because this function
/// does the lookup. `provider.rs`'s strings are different: a `CamelProvider`
/// calls `dgettext` on a `'static` constant.
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use zeroize::Zeroizing;

    use super::{VERIFIER_LIFETIME, Verifiers};

    fn secret(value: &str) -> Zeroizing<String> {
        Zeroizing::new(value.to_owned())
    }

    #[test]
    fn a_verifier_is_redeemed_once_as_a_wiping_secret() {
        let now = Instant::now();
        let mut verifiers = Verifiers::default();
        verifiers.stash("uid".to_owned(), secret("first"), now);
        // The type annotation is the test: a value that leaves the map wipes itself.
        let redeemed: Zeroizing<String> = verifiers.redeem("uid", now).expect("stashed");
        assert_eq!(redeemed.as_str(), "first");
        assert!(verifiers.redeem("uid", now).is_none(), "single-use");
    }

    #[test]
    fn a_repeated_authorization_replaces_the_stashed_verifier() {
        let now = Instant::now();
        let mut verifiers = Verifiers::default();
        verifiers.stash("uid".to_owned(), secret("first"), now);
        verifiers.stash("uid".to_owned(), secret("second"), now);
        assert_eq!(verifiers.len(), 1);
        assert_eq!(
            verifiers.redeem("uid", now).expect("stashed").as_str(),
            "second"
        );
    }

    #[test]
    fn a_verifier_is_redeemable_until_its_lifetime_ends() {
        let then = Instant::now();
        let mut verifiers = Verifiers::default();
        verifiers.stash("uid".to_owned(), secret("first"), then);
        let just_in_time = then + VERIFIER_LIFETIME - Duration::from_secs(1);
        assert!(verifiers.redeem("uid", just_in_time).is_some());

        verifiers.stash("uid".to_owned(), secret("first"), then);
        assert!(
            verifiers.redeem("uid", then + VERIFIER_LIFETIME).is_none(),
            "a verifier at the end of its lifetime belongs to an abandoned consent"
        );
    }

    #[test]
    fn an_abandoned_verifier_is_evicted_by_another_accounts_stash() {
        let then = Instant::now();
        let mut verifiers = Verifiers::default();
        verifiers.stash("abandoned".to_owned(), secret("first"), then);
        verifiers.stash("recent".to_owned(), secret("second"), then);
        let later = then + VERIFIER_LIFETIME;
        verifiers.stash("next".to_owned(), secret("third"), later);
        assert_eq!(verifiers.len(), 1, "only the fresh stash survives");
        assert!(verifiers.redeem("next", later).is_some());
    }

    #[test]
    fn an_abandoned_verifier_is_evicted_by_another_accounts_redeem() {
        let then = Instant::now();
        let mut verifiers = Verifiers::default();
        verifiers.stash("abandoned".to_owned(), secret("first"), then);
        assert!(
            verifiers
                .redeem("other", then + VERIFIER_LIFETIME)
                .is_none()
        );
        assert_eq!(verifiers.len(), 0);
    }

    #[test]
    fn the_lifetime_outlasts_the_recommended_code_lifetime() {
        // RFC 6749 section 4.1.2 recommends ten minutes or less for the code
        // only. The consent page comes before the code.
        assert!(VERIFIER_LIFETIME > Duration::from_secs(10 * 60));
    }
}
