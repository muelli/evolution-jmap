// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelSaslXOAuth2Jmap`: the provider's named authentication mechanism.
//!
//! ## What a named mechanism is *for*
//!
//! Nothing here ever runs a SASL exchange. JMAP authenticates over HTTP — an
//! `Authorization: Bearer` header put on each request by [`crate::connect`] —
//! and there is no SASL round trip for a `CamelSasl` to conduct. What this type
//! exists to do is be *found*, by name, by two lookups in Evolution that decide
//! how an account is treated before any of our code is reached.
//!
//! The first is the one this project has a bug report about
//! (GNOME/evolution#3382). `mail_ui_session_authenticate_sync` (evolution
//! 3.52.4, `src/mail/e-mail-ui-session.c`) branches on the mechanism it was
//! given:
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
//! A NULL mechanism skips that branch entirely and falls through to
//! `e_credentials_prompter_loop_prompt_sync`, which for an OAuth2-method source
//! is the consent window — before the service has been asked even once. That is
//! the "a consent prompt at every send while every silent token path answered
//! fine" the operator observed on 2026-08-26. Milan Crha's answer on the issue
//! is that a NULL mechanism *means* "you do not know how to connect", and that
//! a provider which does should say so with a named `CamelSasl` — as
//! evolution-ews does in `src/EWS/common/camel-sasl-xoauth2-office365.c`.
//!
//! The second lookup is the account editor's, and it wants the same string
//! without being asked: `mail_config_auth_check_host_changed_cb`
//! (`src/mail/e-mail-config-auth-check.c`) does
//!
//! ```c
//! change_authtype = camel_sasl_authtype (e_oauth2_service_get_name (oauth2_service));
//! ```
//!
//! and adds whatever comes back to the *Authentication type* combo. So
//! Evolution already looks a mechanism up under our `EOAuth2Service`'s name and
//! finds nothing today.
//!
//! ## Why the authproto is the whole contract
//!
//! `camel_sasl_authtype` (evolution-data-server 3.52.3, `src/camel/camel-sasl.c`)
//! does not consult a registry a provider adds itself to. It walks
//! `g_type_children (CAMEL_TYPE_SASL)` recursively, class-refs every
//! non-abstract descendant, and files each one under
//! `sasl_class->auth_type->authproto`:
//!
//! ```c
//! key = (gpointer) sasl_class->auth_type->authproto;
//! g_hash_table_insert (class_table, key, sasl_class);
//! ```
//!
//! So *registering the type is the registration*, and the mechanism's name is
//! whatever string the class's [`CamelServiceAuthType`] carries. Both lookups
//! above key on our `EOAuth2Service`'s name, so [`auth_type`]'s `authproto` is
//! [`OAUTH2_SERVICE_NAME`] — the constant lives in `jmap-backend-core` rather
//! than in either crate that spells it, because this crate cannot see
//! `jmap-config` and a literal in each would be two strings nothing compares.
//!
//! Deriving from `CamelSaslXOAuth2` rather than straight from `CamelSasl` is
//! the other half of the naming: `camel_sasl_is_xoauth2_alias` answers by
//! walking a class's parents looking for `CAMEL_IS_SASL_XOAUTH2_CLASS`, and it
//! is what `e_auth_combo_box_update_available` uses to decide that a mechanism
//! under a private name is still the bearer-token one — the difference between
//! the entry being offered and being struck through.
//!
//! ## Why static registration, and not `G_DEFINE_DYNAMIC_TYPE`
//!
//! evolution-ews registers its subclass dynamically, from
//! `module-ews-configuration.c`'s `e_module_load` — it has a `GTypeModule` to
//! register against because that module is an `EModule`. This provider has
//! none: Camel's entry point is `camel_provider_module_init (void)`, with no
//! argument to be one, and Camel never closes a provider module (see
//! [`crate::module`]). So this registers statically, exactly as
//! [`crate::store`] and [`crate::transport`] already do and for the same
//! reason.
//!
//! The Camel module is also the better *site* than an Evolution shell module
//! would be. It is dlopened precisely when something asks for the `jmap`
//! protocol, which is to say whenever a JMAP `CamelService` exists at all —
//! and a JMAP service existing is the only circumstance in which either lookup
//! above is asked about our mechanism.
//!
//! ## What it does not implement
//!
//! Neither `challenge_sync` nor `try_empty_password_sync`. Both are inherited
//! from `CamelSaslXOAuth2`, which builds the RFC 7628 initial response out of
//! `camel_session_get_oauth2_access_token_sync` — correct for a mechanism that
//! is spoken over IMAP or SMTP, and unreachable for one that is not. Nothing in
//! this project instantiates the type: `mail_ui_session_authenticate_sync`'s
//! one-shot branch calls `camel_sasl_authtype` and then goes straight to
//! `camel_service_authenticate_sync`, so what is consulted is the *class*, and
//! `camel_sasl_new` is only reached down the fallback branch this type exists
//! to keep the account out of.

use std::ffi::CStr;
use std::sync::OnceLock;

use eds_sys::{
    CamelSaslClass, CamelSaslXOAuth2, CamelSaslXOAuth2Class, CamelServiceAuthType, CamelSettings,
    camel_sasl_xoauth2_get_type,
};
use glib_sys::{GFALSE, GType, gchar};
use jmap_backend_core::i18n::N_;
use jmap_backend_core::oauth2::OAUTH2_SERVICE_NAME;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};

/// What Evolution's *Authentication type* combo shows for this mechanism.
///
/// Marked with [`N_`] rather than looked up, for the same reason
/// [`crate::provider`]'s `NAME` is: the lookup is Camel's, not ours.
/// `provider_register_internal` (camel-provider.c) translates every entry of a
/// provider's `authtypes` list with the provider's own `translation_domain` as
/// it registers it —
///
/// ```c
/// for (link = provider->authtypes; link != NULL; link = link->next) {
///         CamelServiceAuthType *auth = link->data;
///         auth->name = P_(auth->name);
///         auth->description = P_(auth->description);
/// }
/// ```
///
/// — and `auth_combo_box_rebuild_model` then puts the result into its model
/// verbatim. So the string is translated exactly once, in Camel, in
/// [`DOMAIN`](jmap_backend_core::i18n::DOMAIN), which is why it must reach
/// this module untranslated.
///
/// That loop is also why the struct below cannot be a `static`: Camel
/// *writes* those two fields, so a `CamelServiceAuthType` in `.rodata` is a
/// SIGSEGV inside `camel_provider_register`.
// TRANSLATORS: the name of an authentication method, in the "Authentication
// type" list of a JMAP account's settings. "JMAP" is a protocol name — leave it
// as it is unless your language writes it in another script.
const NAME: &CStr = N_(c"OAuth2 (JMAP)");

/// The one-line description of that entry, translated by the same loop and so
/// marked for the same reason.
///
/// Kept short enough to be one statement on one line, which is not a style
/// preference: `xgettext --add-comments` attaches an extraction comment only
/// to the line the string is *on*, so a wrapped `N_(…)` silently drops the
/// note below and the msgid reaches translators bare. (Do not write that
/// keyword in a doc comment either — `po/extract.sh` reads this file as C, so
/// prose mentioning it is scraped into the catalogue as if it were the note.)
// TRANSLATORS: the description of the "OAuth2 (JMAP)" authentication method.
const DESCRIPTION: &CStr = N_(c"Uses an OAuth 2.0 access token to connect to the JMAP server.");

/// The mechanism as Camel describes it, as a pointer Camel and this module
/// both hold — the same shape, and for the same reasons, as
/// [`crate::provider`]'s own `Registered`.
///
/// The struct is heap-allocated once and leaked. Both halves of that are
/// forced. *Leaked*, because Camel keeps the pointer: it is reached through
/// the class for as long as the type is registered, and `camel_sasl_authtype`
/// hands it out to callers that hold it (`EMailConfigAuthCheck` puts it in a
/// combo's model). *Heap*, because `camel_provider_register` writes the `name`
/// and `description` fields in place when it translates them — see
/// [`NAME`] — so the `static` this would otherwise obviously be
/// would put it in `.rodata` and turn registration into a segfault.
struct AuthType(*mut CamelServiceAuthType);

// SAFETY: the pointer is set once, under the OnceLock, and what it points at
// is leaked — nothing can free it or move it. Camel writes the two
// translatable fields exactly once, from inside `camel_provider_register`,
// which this module reaches from `provider::register` before the provider
// (and so the auth type) is visible to anything else.
unsafe impl Send for AuthType {}
unsafe impl Sync for AuthType {}

static AUTH_TYPE: OnceLock<AuthType> = OnceLock::new();

/// The instance struct. Nothing of our own: the mechanism is a *declaration*,
/// and what declares it is the class.
#[repr(C)]
pub struct JmapSasl {
    parent: CamelSaslXOAuth2,
}

/// The class struct, which is where the declaration lives — in the
/// `auth_type` slot [`class_init`](ObjectSubclass::class_init) fills, three
/// levels up in `CamelSaslClass`.
#[repr(C)]
pub struct JmapSaslClass {
    parent_class: CamelSaslXOAuth2Class,
}

// SAFETY: both structs are #[repr(C)] and lead with the CamelSaslXOAuth2
// instance and class structs, whose layouts eds-sys's tests/layout.rs checks
// against `g_type_query`; CamelSaslXOAuth2 derives from CamelSasl, from
// GObject.
unsafe impl ObjectSubclass for JmapSasl {
    /// `CamelSaslXOAuth2Jmap`, matching evolution-ews's
    /// `CamelSaslXOAuth2Office365` and Camel's own `CamelSaslXOAuth2Google`:
    /// the type name is what a user sees in a GObject warning, and what
    /// `camel_sasl.c`'s "%s has an empty CamelServiceAuthType" critical would
    /// name if the class below ever stopped filling the slot.
    const NAME: &'static CStr = c"CamelSaslXOAuth2Jmap";
    type Instance = JmapSasl;
    type Class = JmapSaslClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { camel_sasl_xoauth2_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // The whole of what this type contributes. Written through
        // `CamelSaslClass`, two levels up, because that is where Camel declares
        // the slot and where `sasl_build_class_table_rec` reads it back from.
        //
        // Overriding rather than filling an empty slot: `CamelSaslXOAuth2`'s
        // own class initialiser puts the generic `"XOAUTH2"` auth type there,
        // which every subclass inherits until it says otherwise. Leaving it
        // would be this type registering itself under a name Camel already has
        // a class for — whichever of the two GLib happened to walk last would
        // win the `"XOAUTH2"` key, and our own name would answer NULL.
        //
        // SAFETY: the class leads with CamelSaslXOAuth2Class, which leads with
        // CamelSaslClass — the contract at the top of this impl. `AUTH_TYPE` is
        // `'static` and never written, so the pointer stays valid for as long
        // as Camel can reach the class.
        let sasl = class.cast::<CamelSaslClass>();
        unsafe { (*sasl).auth_type = auth_type() };
    }
}

/// Registers the mechanism type, or returns it if it is already registered.
///
/// Statically, for the reason the module docs give. Calling this is the entire
/// act of publishing the mechanism — there is no `camel_sasl_register` —
/// so [`crate::provider::register`] calls it, and everything downstream is
/// Camel walking `CAMEL_TYPE_SASL`'s children.
pub fn sasl_type() -> GType {
    register_static::<JmapSasl>()
}

/// The mechanism's name, as the string Camel and Evolution look it up by.
///
/// The same bytes as [`OAUTH2_SERVICE_NAME`], named again here so that a caller
/// asking "what mechanism does this provider offer?" — [`crate::service`]'s
/// `connect_sync`, and this crate's tests — reads it off the mechanism rather
/// than reaching for the OAuth 2.0 service's name and relying on the two being
/// equal by inspection.
pub const MECHANISM: &CStr = OAUTH2_SERVICE_NAME;

/// The mechanism as a pointer to put in [`crate::provider`]'s `authtypes`
/// list, which is how the account editor's combo learns the entry exists.
///
/// One allocation, handed to both consumers.
///
/// That they are the same allocation is not tidiness. Camel translates the
/// provider's copy in place, and Camel's own providers get away with that
/// because the thing in their `authtypes` list *is* the SASL class's auth
/// type — `camel_sasl_authtype_list` returns the classes' own structs. Two
/// copies here would leave the class's untranslated and the combo's
/// translated: same mechanism, two names, and only one of them would match
/// what `camel_sasl_authtype` reports.
///
/// `need_password` is `FALSE`, and that field is the entire point of the
/// exercise — it is what `mail_ui_session_authenticate_sync` tests to decide
/// that this mechanism gets one silent attempt before any prompt. An account
/// whose token is in the keyring never sees a consent window; one whose token
/// the *server* rejects still does, because a rejection is that branch's
/// recovery rather than its failure.
pub fn auth_type() -> *mut CamelServiceAuthType {
    AUTH_TYPE
        .get_or_init(|| {
            AuthType(Box::into_raw(Box::new(CamelServiceAuthType {
                name: NAME.as_ptr(),
                description: DESCRIPTION.as_ptr(),
                // The string both of Evolution's lookups key on — see the
                // module docs.
                authproto: OAUTH2_SERVICE_NAME.as_ptr(),
                need_password: GFALSE,
            })))
        })
        .0
}

/// The mechanism name [`crate::service`]'s `connect_sync` names when it asks
/// the session to authenticate an account configured by `settings` — or NULL,
/// which is Camel's "this account has no mechanism to speak of".
///
/// [`MECHANISM`] for an OAuth 2.0 account, because naming it is what earns the
/// account a silent attempt ahead of any prompt; NULL for a password and for
/// an API token, both of which have to be *typed* and so want the session's
/// prompt-first loop. See the module docs for the branch in
/// `mail_ui_session_authenticate_sync` this decides between.
///
/// The answer is [`MECHANISM`] and not the account's own `auth-mechanism`
/// string, because [`uses_oauth2`] also accepts EDS's generic `"OAuth2"`
/// spelling — a name no `CamelSasl` and no `EOAuth2Service` carries, so
/// passing it through would fail both of the session's lookups. This is a
/// question of *which mechanism this provider speaks*, and the answer to that
/// does not vary with how the account spelled its choice.
///
/// [`uses_oauth2`]: crate::oauth2::uses_oauth2
///
/// # Safety
///
/// `settings` must be NULL or a valid `CamelSettings`. It is only read from,
/// and nothing outlives the call — the string returned is `'static`.
pub unsafe fn mechanism_for(settings: *mut CamelSettings) -> *const gchar {
    // SAFETY: the contract above is `uses_oauth2`'s own.
    if unsafe { crate::oauth2::uses_oauth2(settings) } {
        MECHANISM.as_ptr()
    } else {
        std::ptr::null()
    }
}
