// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where an account's discovered OAuth 2.0 endpoints and client id live on
//! disk: a custom `ESourceExtension`, `[JMAP OAuth2]` in the account's own
//! keyfile.
//!
//! `jmap_client::oauth` can discover a deployment's endpoints (RFC 8414) and
//! register a client id with it (RFC 7591), but both are network calls, and
//! the `EOAuth2Service` interface EDS asks an account's endpoints and client
//! id through — `get_client_id`, `get_authentication_uri`, `get_refresh_uri`
//! — is a set of *synchronous* vfuncs with no way to make one. So discovery
//! and registration have to happen once, ahead of time (most plausibly from
//! M7's setup UI), and the answer has to survive an Evolution restart. This
//! module is where it is kept, so that implementation can be what
//! `eds-sys/tests/oauth2.rs` already expects it to be: ordinary Rust reading
//! a stored value, with none of the per-call ownership questions a `const
//! gchar *` an interface vfunc *computed* would raise.
//!
//! ## A custom extension, not a new field on an existing one
//!
//! `ESource` extensions are found by name: `e_source_get_extension` walks
//! every registered subclass of `ESourceExtension` and matches
//! `ESourceExtensionClass::name` against the string it was asked for — the
//! same mechanism `[Collection]`, `[Authentication]` and `[Security]` are
//! found by in [`crate::account`], and open to third-party types the same
//! way (checked against upstream's `e-source.c`, not assumed: the lookup
//! walks `g_type_children` of `ESourceExtension` and hashes each concrete
//! subclass's `name` field, with no distinction between EDS's own subclasses
//! and anyone else's). Registering [`Extension`] is therefore the whole of
//! "storage this project defines", with none of EDS's own extensions
//! touched — and, as with them, the type has to be registered (referenced)
//! before the first lookup for its name, which [`read`] and [`apply`] do the
//! same way [`crate::account::apply`] does for EDS's own extension types.
//!
//! ## Properties, because that is what persists
//!
//! A GObject property flagged `E_SOURCE_PARAM_SETTING` is what `ESource`'s
//! own (de)serialisation reads and writes — not a convenience, the only path
//! a value here has to the `.source` file on disk. So [`Extension`] installs
//! five string properties under that flag rather than keeping the five
//! fields as plain Rust state nothing outside this process would ever see
//! again after a restart.
//!
//! [`apply`] and [`read`] are this module's own door onto them, and read and
//! write the storage directly rather than going through `g_object_get`/
//! `_set`: unlike [`crate::account`]'s fields, which read and write
//! extensions EDS itself defines and already exposes typed setters for,
//! there is no existing accessor to reuse here, so this module writes both
//! doors onto one storage — its own callers, and `get_property`/
//! `set_property`, which is what EDS's serialisation calls.

use std::ffi::{CStr, CString, c_char};
use std::ptr;
use std::sync::{Mutex, PoisonError};

use eds_sys::{
    ESource, ESourceExtension, ESourceExtensionClass, e_source_extension_get_type,
    e_source_get_extension, e_source_has_extension,
};
use glib_sys::{GFALSE, GType};
use gobject_sys::{
    G_PARAM_READWRITE, G_PARAM_USER_SHIFT, GObject, GObjectClass, GParamFlags, GParamSpec, GValue,
    g_object_class_install_property, g_param_spec_string, g_value_get_string, g_value_set_string,
};
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::instance::Slot;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use jmap_backend_core::trampoline::{guard, log_critical};

/// The keyfile group EDS finds this extension by — `e_source_get_extension`'s
/// `extension_name` argument, and what an account's `.source` file shows as
/// `[JMAP OAuth2]`.
pub const EXTENSION_NAME: &CStr = c"JMAP OAuth2";

/// `E_SOURCE_PARAM_SETTING`, computed rather than bound: `e-source.h` defines
/// it as a plain `#define (1 << G_PARAM_USER_SHIFT)`, not a symbol, so there
/// is nothing for bindgen to hand back.
const E_SOURCE_PARAM_SETTING: GParamFlags = 1 << (G_PARAM_USER_SHIFT as u32);

/// What a JMAP account's `EOAuth2Service` implementation will need per
/// account, once it exists: RFC 8414 discovery's endpoints and RFC 7591
/// registration's client id, plus the redirect URI this client registered
/// with. All five are set together, by whatever performed discovery and
/// registration — there is no partial state for a reader to make sense of,
/// only "not done yet", which is every field absent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// RFC 7591 §3.2.1's `client_id` — absent until registration succeeds.
    pub client_id: Option<String>,
    /// RFC 7591 §3.2.1's `client_secret`, present only if the server issued
    /// one despite this client registering as public
    /// (`jmap_client::oauth::CLIENT_AUTH_METHOD`).
    pub client_secret: Option<String>,
    /// RFC 8414 §2's `authorization_endpoint` —
    /// `EOAuth2Service::get_authentication_uri`'s eventual answer.
    pub authorization_endpoint: Option<String>,
    /// RFC 8414 §2's `token_endpoint` — both where a code is redeemed and
    /// `EOAuth2Service::get_refresh_uri`'s eventual answer, RFC 6749 having
    /// one endpoint serve what EDS asks about as two.
    pub token_endpoint: Option<String>,
    /// RFC 6749 §3.1.2's `redirect_uri`, fixed at registration time and asked
    /// for again on every authentication — `get_redirect_uri`.
    pub redirect_uri: Option<String>,
}

/// The instance struct: nothing but [`ESourceExtension`]'s own state plus one
/// [`Slot`] for the five values, following the layout contract
/// [`ObjectSubclass`] requires.
#[repr(C)]
pub struct Extension {
    parent: ESourceExtension,
    fields: Slot<Mutex<Fields>>,
}

/// The class struct: nothing but [`ESourceExtensionClass`]'s own state.
/// `class_init` reaches into the leading `name` field through it, and into
/// `GObjectClass`'s `get_property`/`set_property` through *that*.
#[repr(C)]
pub struct ExtensionClass {
    parent_class: ESourceExtensionClass,
}

/// The five values, owned as `CString`s so `get_property` can hand a borrowed
/// pointer straight to `g_value_set_string` (which copies it) without an
/// allocation on every read.
#[derive(Default)]
struct Fields {
    client_id: Option<CString>,
    client_secret: Option<CString>,
    authorization_endpoint: Option<CString>,
    token_endpoint: Option<CString>,
    redirect_uri: Option<CString>,
}

/// The five properties' ids and the names `class_init` installs them under —
/// dense from 1, since GObject treats 0 as "no property", and local to this
/// class alone.
const PROPERTIES: [(u32, &CStr); 5] = [
    (1, c"client-id"),
    (2, c"client-secret"),
    (3, c"authorization-endpoint"),
    (4, c"token-endpoint"),
    (5, c"redirect-uri"),
];

const PROP_CLIENT_ID: u32 = PROPERTIES[0].0;
const PROP_CLIENT_SECRET: u32 = PROPERTIES[1].0;
const PROP_AUTHORIZATION_ENDPOINT: u32 = PROPERTIES[2].0;
const PROP_TOKEN_ENDPOINT: u32 = PROPERTIES[3].0;
const PROP_REDIRECT_URI: u32 = PROPERTIES[4].0;

// SAFETY: both structs are #[repr(C)] and lead with ESourceExtension's own
// instance/class structs, whose layout eds-sys's tests/layout.rs checks
// against `g_type_query`; ESourceExtension derives from GObject.
unsafe impl ObjectSubclass for Extension {
    const NAME: &'static CStr = c"JmapOAuth2Extension";
    type Instance = Extension;
    type Class = ExtensionClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_source_extension_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` leads with `ESourceExtensionClass`, which the
        // contract above pins — this is the same field
        // `source_find_extension_classes_rec` reads to match an extension
        // name to a `GType`.
        unsafe { (*class).parent_class.name = EXTENSION_NAME.as_ptr() };

        // SAFETY: `ExtensionClass` leads with `ESourceExtensionClass`, which
        // leads with `GObjectClass` — the same transitive leading-field cast
        // `jmap_mail::settings` uses for `CamelJmapSettingsClass`.
        let object_class = class.cast::<GObjectClass>();
        unsafe {
            (*object_class).set_property = Some(set_property);
            (*object_class).get_property = Some(get_property);

            for (id, name) in PROPERTIES {
                g_object_class_install_property(
                    object_class,
                    id,
                    g_param_spec_string(
                        name.as_ptr(),
                        ptr::null(),
                        ptr::null(),
                        ptr::null(),
                        G_PARAM_READWRITE | E_SOURCE_PARAM_SETTING,
                    ),
                );
            }
        }
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // SAFETY: a freshly zeroed instance — the slot's own empty state.
        unsafe { (*instance).fields.init(Mutex::new(Fields::default())) };
    }

    unsafe fn finalize(instance: *mut Self::Instance) {
        // SAFETY: the last reference is gone; nothing else can reach `fields`.
        unsafe { (*instance).fields.clear() };
    }
}

/// Routes a property write into the instance's own storage.
///
/// Guarded like every other function C calls into this crate: this runs
/// inside `g_object_set`, which EDS's own `.source` parser reaches when it
/// restores an account, and a panic unwinding out of it is undefined
/// behaviour in a process full of other people's mail.
unsafe extern "C" fn set_property(
    object: *mut GObject,
    id: u32,
    value: *mut GValue,
    _pspec: *mut GParamSpec,
) {
    guard("JmapOAuth2Extension::set_property", (), || {
        // SAFETY: GObject passes an instance of this type; `instance_init`
        // has already run by the time any vfunc can be dispatched on it.
        let fields = unsafe { &*object.cast::<Extension>() }
            .fields
            .get()
            .expect("set_property dispatched before instance_init ran");
        let mut fields = fields.lock().unwrap_or_else(PoisonError::into_inner);
        // SAFETY: `value` holds the string type every pspec above declares.
        let text = unsafe { read_cstring(value) };
        match id {
            PROP_CLIENT_ID => fields.client_id = text,
            PROP_CLIENT_SECRET => fields.client_secret = text,
            PROP_AUTHORIZATION_ENDPOINT => fields.authorization_endpoint = text,
            PROP_TOKEN_ENDPOINT => fields.token_endpoint = text,
            PROP_REDIRECT_URI => fields.redirect_uri = text,
            _ => log_critical(&format!(
                "JmapOAuth2Extension has no property with id {id}; the value is dropped"
            )),
        }
    });
}

/// The reading half, symmetric with [`set_property`].
unsafe extern "C" fn get_property(
    object: *mut GObject,
    id: u32,
    value: *mut GValue,
    _pspec: *mut GParamSpec,
) {
    guard("JmapOAuth2Extension::get_property", (), || {
        // SAFETY: as in `set_property`.
        let fields = unsafe { &*object.cast::<Extension>() }
            .fields
            .get()
            .expect("get_property dispatched before instance_init ran");
        let fields = fields.lock().unwrap_or_else(PoisonError::into_inner);
        let text = match id {
            PROP_CLIENT_ID => fields.client_id.as_deref(),
            PROP_CLIENT_SECRET => fields.client_secret.as_deref(),
            PROP_AUTHORIZATION_ENDPOINT => fields.authorization_endpoint.as_deref(),
            PROP_TOKEN_ENDPOINT => fields.token_endpoint.as_deref(),
            PROP_REDIRECT_URI => fields.redirect_uri.as_deref(),
            _ => {
                log_critical(&format!(
                    "JmapOAuth2Extension has no property with id {id}; nothing is read"
                ));
                return;
            }
        };
        // SAFETY: `value` is the `GValue` GObject is filling for this get;
        // `g_value_set_string` copies whatever `text` points at before this
        // call returns, so the borrow on `fields` does not need to outlive it.
        unsafe { g_value_set_string(value, text.map_or(ptr::null(), CStr::as_ptr)) };
    });
}

/// Copies a `GValue`'s string, or `None` for a NULL one — a cleared property
/// is exactly how a caller un-sets a field, the same as every string setter
/// in [`crate::account`].
///
/// # Safety
///
/// `value` holds a `G_TYPE_STRING` `GValue`, valid for the call.
unsafe fn read_cstring(value: *const GValue) -> Option<CString> {
    // SAFETY: forwarded from the caller's contract; the returned pointer is
    // borrowed for this call only, which is why it is copied below rather
    // than kept.
    let text = unsafe { g_value_get_string(value) };
    if text.is_null() {
        return None;
    }
    // SAFETY: a non-NULL result is a NUL-terminated string valid for this
    // call.
    Some(unsafe { CStr::from_ptr(text) }.to_owned())
}

/// Registers [`Extension`] (or finds it already registered), which is what
/// makes it one `e_source_get_extension`/`e_source_has_extension` can match
/// `EXTENSION_NAME` against — an unregistered type is invisible to the
/// lookup the same way an unregistered EDS extension type would be, per
/// `e-source.c`'s `source_find_extension_classes_rec`.
///
/// [`apply`] and [`read`] call this before touching a source, so any caller
/// that only goes through them never has to think about it — the same way a
/// caller of `account::apply` never registers `ESourceCollection`'s `GType`
/// by hand. The one caller that *does* have to think about it is whatever
/// eventually loads this project's EDS module: EDS's own `.source` parser
/// restores every group's extension by the same name lookup, so this has to
/// have run — once, at module load, alongside registering the module's other
/// types — before EDS is asked to parse a keyfile that carries `[JMAP
/// OAuth2]`, or that group is silently unrecognised rather than restored.
pub fn ensure_registered() {
    register_static::<Extension>();
}

/// `source`'s `[JMAP OAuth2]` extension, registering the type and creating
/// the group if neither exists yet — `e_source_get_extension`'s own "absent
/// means create it" rule, the same one [`crate::account::apply`] relies on
/// for EDS's extensions.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
unsafe fn extension_of(source: *mut ESource) -> *mut Extension {
    ensure_registered();
    // SAFETY: `source` is valid by this function's contract, and the type is
    // registered by the line above.
    unsafe { e_source_get_extension(source, EXTENSION_NAME.as_ptr()) }.cast()
}

/// Writes `config` onto `source`'s `[JMAP OAuth2]` extension, creating it if
/// this is the first thing to touch it. Every field is written, `None`
/// clearing whatever was there before — the same idempotent-rewrite rule
/// [`crate::account::apply`] follows, and for the same reason: this runs
/// again whenever discovery or registration is redone, not only once.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn apply(source: *mut ESource, config: &Config) {
    // SAFETY: `source` is valid by this function's contract.
    let extension = unsafe { extension_of(source) };
    // SAFETY: `extension_of` only ever returns an instance of this type,
    // whose `instance_init` has already run.
    let fields = unsafe { &*extension }
        .fields
        .get()
        .expect("e_source_get_extension returned an instance instance_init did not run on");
    *fields.lock().unwrap_or_else(PoisonError::into_inner) = Fields {
        client_id: config.client_id.as_deref().map(cstring_lossy),
        client_secret: config.client_secret.as_deref().map(cstring_lossy),
        authorization_endpoint: config.authorization_endpoint.as_deref().map(cstring_lossy),
        token_endpoint: config.token_endpoint.as_deref().map(cstring_lossy),
        redirect_uri: config.redirect_uri.as_deref().map(cstring_lossy),
    };
}

/// The config `source`'s `[JMAP OAuth2]` extension currently says — every
/// field absent for a source nothing has written to yet, since reading must
/// not create what [`apply`] did not.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn read(source: *mut ESource) -> Config {
    ensure_registered();

    // SAFETY: `source` is valid by this function's contract.
    if unsafe { e_source_has_extension(source, EXTENSION_NAME.as_ptr()) } == GFALSE {
        return Config::default();
    }

    // SAFETY: the extension is present, so this returns the source's own,
    // whose `instance_init` has already run.
    let extension =
        unsafe { e_source_get_extension(source, EXTENSION_NAME.as_ptr()) }.cast::<Extension>();
    let fields = unsafe { &*extension }
        .fields
        .get()
        .expect("e_source_get_extension returned an instance instance_init did not run on");
    let fields = fields.lock().unwrap_or_else(PoisonError::into_inner);
    Config {
        client_id: fields
            .client_id
            .as_ref()
            .map(|value| value.to_string_lossy().into_owned()),
        client_secret: fields
            .client_secret
            .as_ref()
            .map(|value| value.to_string_lossy().into_owned()),
        authorization_endpoint: fields
            .authorization_endpoint
            .as_ref()
            .map(|value| value.to_string_lossy().into_owned()),
        token_endpoint: fields
            .token_endpoint
            .as_ref()
            .map(|value| value.to_string_lossy().into_owned()),
        redirect_uri: fields
            .redirect_uri
            .as_ref()
            .map(|value| value.to_string_lossy().into_owned()),
    }
}

// ---------------------------------------------------------------------------
// Borrowed access, for `EOAuth2Service`'s vfuncs.

/// One field of `source`'s `[JMAP OAuth2]` extension, borrowed rather than
/// copied — a `const gchar *` into the extension's own storage, or NULL for a
/// source nothing has written to (or that field of) yet.
///
/// This is not [`read`]'s shape on purpose. `EOAuth2Service`'s vfuncs —
/// `get_client_id` and its four siblings this module exists for — return
/// `const gchar *`, and [`read`]'s owned `String`s are dropped the moment the
/// vfunc returns, which would leave the caller holding a dangling pointer the
/// instant it looked at it. What is returned here instead is a pointer into
/// the `CString` [`Fields`] already owns: stable for as long as the
/// extension is, i.e. for as long as the account's `ESource` is, which is far
/// longer than any single vfunc dispatch needs. That is the same contract
/// EDS's own extensions keep for their string accessors (`
/// e_source_authentication_get_host` and the rest) with no lock of their own
/// either — mutating a source concurrently with reading it is a caller error
/// generally, not a hazard specific to this one.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
unsafe fn borrowed(
    source: *mut ESource,
    field: impl FnOnce(&Fields) -> &Option<CString>,
) -> *const c_char {
    ensure_registered();

    // SAFETY: `source` is valid by this function's contract.
    if unsafe { e_source_has_extension(source, EXTENSION_NAME.as_ptr()) } == GFALSE {
        return ptr::null();
    }

    // SAFETY: the extension is present, so this returns the source's own,
    // whose `instance_init` has already run.
    let extension =
        unsafe { e_source_get_extension(source, EXTENSION_NAME.as_ptr()) }.cast::<Extension>();
    let fields = unsafe { &*extension }
        .fields
        .get()
        .expect("e_source_get_extension returned an instance instance_init did not run on");
    let fields = fields.lock().unwrap_or_else(PoisonError::into_inner);
    field(&fields).as_deref().map_or(ptr::null(), CStr::as_ptr)
}

/// `source`'s stored RFC 7591 `client_id`, or NULL before registration has
/// run. See [`borrowed`] for the pointer's lifetime.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn client_id(source: *mut ESource) -> *const c_char {
    unsafe { borrowed(source, |fields| &fields.client_id) }
}

/// `source`'s stored RFC 7591 `client_secret`, or NULL — absent for the
/// ordinary case of a public client, and before registration has run. See
/// [`borrowed`] for the pointer's lifetime.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn client_secret(source: *mut ESource) -> *const c_char {
    unsafe { borrowed(source, |fields| &fields.client_secret) }
}

/// `source`'s stored RFC 8414 `authorization_endpoint`, or NULL before
/// discovery has run. See [`borrowed`] for the pointer's lifetime.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn authorization_endpoint(source: *mut ESource) -> *const c_char {
    unsafe { borrowed(source, |fields| &fields.authorization_endpoint) }
}

/// `source`'s stored RFC 8414 `token_endpoint`, or NULL before discovery has
/// run. See [`borrowed`] for the pointer's lifetime.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn token_endpoint(source: *mut ESource) -> *const c_char {
    unsafe { borrowed(source, |fields| &fields.token_endpoint) }
}

/// `source`'s registered `redirect_uri`, or NULL before registration has run.
/// See [`borrowed`] for the pointer's lifetime.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn redirect_uri(source: *mut ESource) -> *const c_char {
    unsafe { borrowed(source, |fields| &fields.redirect_uri) }
}
