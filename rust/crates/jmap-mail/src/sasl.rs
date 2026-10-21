// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelSaslXOAuth2Jmap`: the named authentication mechanism of the provider.
//!
//! ## What a named mechanism is *for*
//!
//! This module never runs a SASL exchange. JMAP authenticates over HTTP with an
//! `Authorization: Bearer` header that [`crate::connect`] puts on each request.
//! Thus a `CamelSasl` has no SASL round trip to do. This type exists so that
//! two lookups in Evolution *find* it by name. These lookups decide how
//! Evolution treats an account before Evolution calls our code.
//!
//! The first lookup is the cause of the bug report GNOME/evolution#3382.
//! `mail_ui_session_authenticate_sync` (evolution 3.52.4,
//! `src/mail/e-mail-ui-session.c`) branches on the mechanism that it gets:
//!
//! ```c
//! if (mechanism != NULL)
//!         authtype = camel_sasl_authtype (mechanism);
//!
//! /* If the SASL mechanism does not involve a user
//!  * password, then it gets one shot to authenticate. */
//! if (authtype != NULL && !authtype->need_password) {
//!         result = camel_service_authenticate_sync (service, mechanism, …);
//!         if ((result == CAMEL_AUTHENTICATION_REJECTED || …) &&
//!             e_oauth2_services_is_oauth2_alias (…, mechanism)) {
//!                 /* the consent window, as the *recovery* */
//!         }
//!         …
//! }
//! ```
//!
//! With a NULL mechanism, the code skips that branch and goes to
//! `e_credentials_prompter_loop_prompt_sync`. For an OAuth2-method source, that
//! function shows the consent window before the service gets one attempt.
//! The bug report describes this: a consent window at each send, while each
//! silent token path gave a correct answer. Milan Crha answered on the issue
//! that a NULL mechanism *means* "you do not know how to connect". A provider
//! that knows must tell Evolution with a named `CamelSasl`, as evolution-ews
//! does in `src/EWS/common/camel-sasl-xoauth2-office365.c`.
//!
//! The second lookup is in the account editor, and it uses the same string
//! without a request. `mail_config_auth_check_host_changed_cb`
//! (`src/mail/e-mail-config-auth-check.c`) does
//!
//! ```c
//! change_authtype = camel_sasl_authtype (e_oauth2_service_get_name (oauth2_service));
//! ```
//!
//! and adds the result to the *Authentication type* combo. Thus Evolution
//! already searches for a mechanism under the name of our `EOAuth2Service`.
//! Without this type, it finds nothing.
//!
//! ## Why the authproto is the whole contract
//!
//! `camel_sasl_authtype` (evolution-data-server 3.52.3, `src/camel/camel-sasl.c`)
//! does not use a registry that a provider adds itself to. It walks
//! `g_type_children (CAMEL_TYPE_SASL)` recursively, refs the class of each
//! non-abstract descendant, and keys each class on
//! `sasl_class->auth_type->authproto`:
//!
//! ```c
//! key = (gpointer) sasl_class->auth_type->authproto;
//! g_hash_table_insert (class_table, key, sasl_class);
//! ```
//!
//! Thus *the type registration is the mechanism registration*. The name of the
//! mechanism is the string in the [`CamelServiceAuthType`] of the class. Both
//! lookups above key on the OAuth 2.0 method of the account, so the `authproto`
//! of [`auth_type`] is `OAUTH2_AUTH_METHOD`. Without `EOAuth2ServiceDynamic`,
//! that is [`jmap_backend_core::oauth2::OAUTH2_SERVICE_NAME`], the name of our
//! `EOAuth2Service`. The constant is in `jmap-backend-core`, not in a crate that
//! uses it: this crate cannot see `jmap-config`, and a literal in each crate
//! gives two strings that nothing compares.
//!
//! The type derives from `CamelSaslXOAuth2`, not directly from `CamelSasl`, for
//! the other half of the naming. `camel_sasl_is_xoauth2_alias` walks the parents
//! of a class to find `CAMEL_IS_SASL_XOAUTH2_CLASS`.
//! `e_auth_combo_box_update_available` uses it to decide that a mechanism with a
//! private name is still the bearer-token mechanism. This decides if the combo
//! offers the entry or shows it struck through.
//!
//! ## Why static registration, and not `G_DEFINE_DYNAMIC_TYPE`
//!
//! evolution-ews registers its subclass dynamically, from `e_module_load` in
//! `module-ews-configuration.c`. That module is an `EModule`, so it has a
//! `GTypeModule` to register against. This provider has none: the Camel entry
//! point is `camel_provider_module_init (void)`, with no argument that can be
//! one, and Camel never closes a provider module (see [`crate::module`]). Thus
//! this type registers statically, as [`crate::store`] and [`crate::transport`]
//! do, for the same reason.
//!
//! The Camel module is also a better location than an Evolution shell module.
//! Camel dlopens it exactly when something asks for the `jmap` protocol, thus
//! whenever a JMAP `CamelService` exists. Only in that condition do the two
//! lookups above ask about our mechanism.
//!
//! ## What it does not implement
//!
//! This type implements neither `challenge_sync` nor `try_empty_password_sync`.
//! It inherits both from `CamelSaslXOAuth2`, which makes the RFC 7628 initial
//! response from `camel_session_get_oauth2_access_token_sync`. That is correct
//! for a mechanism that IMAP or SMTP uses, and unreachable for this mechanism.
//! Nothing in this project makes an instance of the type. The one-shot branch
//! of `mail_ui_session_authenticate_sync` calls `camel_sasl_authtype` and then
//! `camel_service_authenticate_sync`, so Evolution reads only the *class*. Only
//! the fallback branch calls `camel_sasl_new`, and this type exists to keep the
//! account out of that branch.

use std::ffi::CStr;
#[cfg(not(eds_oauth2_dynamic))]
use std::sync::OnceLock;

use eds_sys::{CamelSaslClass, CamelServiceAuthType, CamelSettings};
#[cfg(not(eds_oauth2_dynamic))]
use eds_sys::{CamelSaslXOAuth2, CamelSaslXOAuth2Class, camel_sasl_xoauth2_get_type};
#[cfg(not(eds_oauth2_dynamic))]
use glib_sys::GFALSE;
use glib_sys::{GType, gchar};
#[cfg(not(eds_oauth2_dynamic))]
use jmap_backend_core::forever::Forever;
#[cfg(not(eds_oauth2_dynamic))]
use jmap_backend_core::i18n::N_;
use jmap_backend_core::oauth2::OAUTH2_AUTH_METHOD;
#[cfg(not(eds_oauth2_dynamic))]
use jmap_backend_core::subclass::{ObjectSubclass, register_static};

/// The name of this mechanism in the *Authentication type* combo of Evolution.
///
/// Marked with [`N_`] and not translated here, for the same reason as `NAME` in
/// [`crate::provider`]: Camel does the translation. `provider_register_internal`
/// (camel-provider.c) translates each entry in the `authtypes` list of a
/// provider with the `translation_domain` of that provider when it registers it:
///
/// ```c
/// for (link = provider->authtypes; link != NULL; link = link->next) {
///         CamelServiceAuthType *auth = link->data;
///         auth->name = P_(auth->name);
///         auth->description = P_(auth->description);
/// }
/// ```
///
/// Then `auth_combo_box_rebuild_model` puts the result into its model without
/// change. Thus Camel translates the string exactly once, in
/// [`DOMAIN`](jmap_backend_core::i18n::DOMAIN), so the string must come to
/// this module untranslated.
///
/// That loop is also why the struct below cannot be a `static`. Camel *writes*
/// those two fields, so a `CamelServiceAuthType` in `.rodata` causes a SIGSEGV
/// in `camel_provider_register`.
// TRANSLATORS: the name of an authentication method, in the "Authentication
// type" list of the settings of a JMAP account. "JMAP" is a protocol name. Do
// not change it, unless your language writes it in a different script.
#[cfg(not(eds_oauth2_dynamic))]
const NAME: &CStr = N_(c"OAuth2 (JMAP)");

/// The one-line description of that entry. The same loop translates it, so it
/// has the same mark for the same reason.
///
/// Keep it short enough for one statement on one line. This is not a style
/// preference: `xgettext --add-comments` attaches an extraction comment only to
/// the line that holds the string. A wrapped `N_(…)` silently loses the note
/// below, and translators get the msgid without a note. (Also, do not write
/// that keyword in a doc comment: `po/extract.sh` reads this file as C, and
/// takes such text into the catalogue as if it were the note.)
// TRANSLATORS: the description of the "OAuth2 (JMAP)" authentication method.
#[cfg(not(eds_oauth2_dynamic))]
const DESCRIPTION: &CStr = N_(c"Uses an OAuth 2.0 access token to connect to the JMAP server.");

/// The mechanism as Camel describes it. Camel and this module both hold this
/// pointer, as with the registered provider of [`crate::provider`].
///
/// The struct is allocated on the heap once and leaked. Both choices are
/// necessary. *Leaked*, because Camel keeps the pointer: the class gives access
/// to it while the type is registered, and `camel_sasl_authtype` gives it to
/// callers that keep it (`EMailConfigAuthCheck` puts it in the model of a
/// combo). *Heap*, because `camel_provider_register` writes the `name` and
/// `description` fields in place when it translates them (see [`NAME`]). A
/// `static` puts the struct in `.rodata`, and registration then segfaults.
#[cfg(not(eds_oauth2_dynamic))]
static AUTH_TYPE: OnceLock<Forever<CamelServiceAuthType>> = OnceLock::new();

/// The instance struct. It has no fields of its own: the mechanism is a
/// *declaration*, and the class makes it.
#[cfg(not(eds_oauth2_dynamic))]
#[repr(C)]
pub struct JmapSasl {
    parent: CamelSaslXOAuth2,
}

/// The class struct. It holds the declaration in the `auth_type` slot of
/// `CamelSaslClass`, two levels up, which [`class_init`](ObjectSubclass::class_init) fills.
#[cfg(not(eds_oauth2_dynamic))]
#[repr(C)]
pub struct JmapSaslClass {
    parent_class: CamelSaslXOAuth2Class,
}

// SAFETY: both structs are #[repr(C)] and start with the CamelSaslXOAuth2
// instance and class structs. tests/layout.rs in eds-sys checks those layouts
// against `g_type_query`. CamelSaslXOAuth2 derives from CamelSasl, which
// derives from GObject.
#[cfg(not(eds_oauth2_dynamic))]
unsafe impl ObjectSubclass for JmapSasl {
    /// `CamelSaslXOAuth2Jmap`, as `CamelSaslXOAuth2Office365` of evolution-ews
    /// and `CamelSaslXOAuth2Google` of Camel. A user sees the type name in a
    /// GObject warning. The "%s has an empty CamelServiceAuthType" critical of
    /// `camel_sasl.c` also shows it if the class below stops filling the slot.
    const NAME: &'static CStr = c"CamelSaslXOAuth2Jmap";
    type Instance = JmapSasl;
    type Class = JmapSaslClass;

    fn parent_type() -> GType {
        // SAFETY: the function takes no arguments, and the type initialises itself.
        unsafe { camel_sasl_xoauth2_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // Camel declares the slot in `CamelSaslClass`, two levels up, and
        // `sasl_build_class_table_rec` reads it there.
        //
        // This replaces a value: `CamelSaslXOAuth2` puts the generic `"XOAUTH2"`
        // auth type in the slot, and each subclass inherits it until it
        // replaces it. If it stays, the `"XOAUTH2"` key goes to the class that
        // GLib walks last, and our own name gives NULL.
        //
        // SAFETY: the class starts with CamelSaslXOAuth2Class, which starts
        // with CamelSaslClass (the contract at the top of this impl).
        // `AUTH_TYPE` is `'static` and never reassigned, so the pointer stays
        // valid while Camel can reach the class.
        let sasl = class.cast::<CamelSaslClass>();
        unsafe { (*sasl).auth_type = auth_type() };
    }
}

/// Registers the mechanism type, or returns it if it is already registered.
///
/// The registration is static, for the reason in the module docs. This call is
/// all of the publication of the mechanism (there is no `camel_sasl_register`).
/// Thus [`crate::provider::register`] calls it, and then Camel walks the
/// children of `CAMEL_TYPE_SASL`.
#[cfg(not(eds_oauth2_dynamic))]
pub fn sasl_type() -> GType {
    register_static::<JmapSasl>()
}

/// Where EDS has `CamelSaslXOAuth2Dynamic`, the mechanism is the upstream type,
/// and this provider declares no type of its own. That type already carries the
/// `authproto` that [`MECHANISM`] is in this build. Upstream's
/// `EOAuth2ServiceDynamic` claims the accounts of this provider under the same
/// name. Two classes under one `authproto` cause the collision that the
/// `class_init` of the other arm prevents: the `class_table` of `camel_sasl.c`
/// keeps the class that `g_type_children` gives last.
///
/// See the `#[cfg(not(eds_oauth2_dynamic))]` arm above for the rest: the
/// registration is still all of the publication, because there is no
/// `camel_sasl_register`.
#[cfg(eds_oauth2_dynamic)]
pub fn sasl_type() -> GType {
    // SAFETY: the function takes no arguments, and the type registers itself on the first call.
    unsafe { eds_sys::camel_sasl_xoauth2_dynamic_get_type() }
}

/// The mechanism name: the string that Camel and Evolution use to find the
/// mechanism.
///
/// It has the same bytes as [`OAUTH2_AUTH_METHOD`]. It has a second name here
/// so that a caller that asks "what mechanism does this provider offer?"
/// ([`crate::service`]'s `connect_sync`, and the tests of this crate) reads it
/// from the mechanism. Thus the caller does not use the method of the account
/// and rely on the two being equal. They must be equal: both lookups in
/// Evolution start from the method of the account, and the second
/// (`mail_config_auth_check_host_changed_cb`) passes `e_oauth2_service_get_name`
/// directly to `camel_sasl_authtype`.
pub const MECHANISM: &CStr = OAUTH2_AUTH_METHOD;

/// The mechanism as a pointer for the `authtypes` list of [`crate::provider`].
/// That list tells the combo of the account editor that the entry exists.
///
/// The class and the list get one allocation.
///
/// This is necessary, not tidiness. Camel translates the copy of the provider
/// in place. The Camel providers can do this because the item in their
/// `authtypes` list *is* the auth type of the SASL class:
/// `camel_sasl_authtype_list` returns the structs of the classes. With two
/// copies here, the copy of the class stays untranslated and the copy of the
/// combo is translated. Then one mechanism has two names, and only one of them
/// matches the result of `camel_sasl_authtype`.
///
/// `need_password` is `FALSE`, and this field is the main purpose of this type.
/// `mail_ui_session_authenticate_sync` tests it to give this mechanism one
/// silent attempt before any prompt. An account with its token in the keyring
/// never sees a consent window. An account whose token the *server* rejects
/// still sees one, because that branch shows the window as its recovery from a
/// rejection, not as a failure.
#[cfg(not(eds_oauth2_dynamic))]
pub fn auth_type() -> *mut CamelServiceAuthType {
    AUTH_TYPE
        .get_or_init(|| {
            // SAFETY: the struct is leaked, so nothing can free or move it.
            // Camel writes the two translatable fields exactly once, in
            // `camel_provider_register`. This module calls that function from
            // `provider::register` before anything else can see the provider
            // (and thus the auth type).
            unsafe {
                Forever::new(Box::into_raw(Box::new(CamelServiceAuthType {
                    name: NAME.as_ptr(),
                    description: DESCRIPTION.as_ptr(),
                    // The key of both lookups in Evolution (see the module docs).
                    authproto: OAUTH2_AUTH_METHOD.as_ptr(),
                    need_password: GFALSE,
                })))
            }
        })
        .as_ptr()
}

/// Where the mechanism is from upstream, the struct that describes it is also
/// from upstream. `camel_sasl_xoauth2_dynamic_class_init` puts its own
/// file-static `CamelServiceAuthType` in the class slot, and the table of
/// `camel_sasl_authtype` already keys that struct under [`MECHANISM`].
///
/// This function reads the struct from the class and makes no second struct.
/// This is the "one allocation, both consumers" rule of the other arm, and here
/// it is necessary: with our own copy, the Camel table points to the upstream
/// struct and the combo of the account editor points to ours. Then one
/// mechanism has two rows, and only one of them matches.
///
/// `g_type_class_ref` has no matching unref, intentionally: the struct must
/// stay valid while the provider holds a pointer into it, which is for the life
/// of the process.
#[cfg(eds_oauth2_dynamic)]
pub fn auth_type() -> *mut CamelServiceAuthType {
    // SAFETY: `sasl_type` registers the type, so `g_type_class_ref` returns its
    // initialised class. That class starts with `CamelSaslXOAuth2Class`, which
    // starts with `CamelSaslClass`, where Camel declares the slot. This is the
    // same transitive leading-field cast that the `class_init` of the other arm
    // writes through.
    unsafe {
        let class = gobject_sys::g_type_class_ref(sasl_type()).cast::<CamelSaslClass>();
        (*class).auth_type
    }
}

/// The mechanism name that [`crate::service`]'s `connect_sync` gives when it
/// asks the session to authenticate an account configured by `settings`, or
/// NULL. For Camel, NULL means "this account has no mechanism".
///
/// [`MECHANISM`] for an OAuth 2.0 account, because the name gives the account a
/// silent attempt before any prompt. NULL for a password and for an API token:
/// the user must *type* both, so they need the prompt-first loop of the session.
/// The module docs show the branch in `mail_ui_session_authenticate_sync` that
/// this choice controls.
///
/// The result is [`MECHANISM`] and not the `auth-mechanism` string of the
/// account, because [`uses_oauth2`] also accepts the generic EDS spelling
/// `"OAuth2"`. No `CamelSasl` and no `EOAuth2Service` has that name, so with it
/// both lookups of the session fail. The question is *which mechanism this
/// provider uses*, and the answer does not change with the spelling in the
/// account.
///
/// [`uses_oauth2`]: crate::oauth2::uses_oauth2
///
/// # Safety
///
/// `settings` must be NULL or a valid `CamelSettings`. This function only reads
/// it, and nothing outlives the call: the returned string is `'static`.
pub unsafe fn mechanism_for(settings: *mut CamelSettings) -> *const gchar {
    // SAFETY: the contract above is the contract of `uses_oauth2`.
    if unsafe { crate::oauth2::uses_oauth2(settings) } {
        MECHANISM.as_ptr()
    } else {
        std::ptr::null()
    }
}
