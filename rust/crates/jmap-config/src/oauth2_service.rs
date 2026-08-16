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
//! ## What this does not yet do
//!
//! Set `[Authentication] method` to [`NAME`] anywhere. That is the setup UI's
//! job (M7), which does not write OAuth2 accounts yet, so an account reaches
//! this service only through a hand-edited `.source` keyfile today.
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

use std::ffi::{CStr, c_char};
use std::ptr;

use eds_sys::{
    EOAuth2Service, EOAuth2ServiceBase, EOAuth2ServiceBaseClass, EOAuth2ServiceInterface, ESource,
    e_oauth2_service_base_get_type, e_oauth2_service_get_type,
};
use glib_sys::GType;
use jmap_backend_core::i18n::{self, N_};
use jmap_backend_core::subclass::{InterfaceDecl, InterfaceImpl, ObjectSubclass};
use jmap_backend_core::trampoline::guard;

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
    }
}

unsafe extern "C" fn get_name(_service: *mut EOAuth2Service) -> *const c_char {
    NAME.as_ptr()
}

/// The one user-visible string here — the account editor's label for this
/// service — so it goes through [`i18n::translate_static`] rather than
/// answering [`NAME`] itself. Marked `N_` for `xgettext` since the lookup is
/// this function's own, not a `CamelProvider`'s `dgettext` call on a
/// `'static` constant the way `provider.rs`'s strings are.
unsafe extern "C" fn get_display_name(_service: *mut EOAuth2Service) -> *const c_char {
    i18n::translate_static(N_(c"JMAP"))
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
