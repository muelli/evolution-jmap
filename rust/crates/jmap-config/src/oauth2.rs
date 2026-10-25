// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The on-disk storage of the discovered OAuth 2.0 endpoints and client id of
//! an account: a custom `ESourceExtension`, `[JMAP OAuth2]` in the keyfile of
//! the account.
//!
//! `jmap_client::oauth` can discover the endpoints of a deployment (RFC 8414)
//! and register a client id with it (RFC 7591), but both are network calls.
//! EDS gets the endpoints and client id of an account through the
//! `EOAuth2Service` interface (`get_client_id`, `get_authentication_uri`,
//! `get_refresh_uri`). These are *synchronous* vfuncs, so they cannot make a
//! network call. Thus discovery and registration occur one time, before
//! authentication ([`crate::oauth2_setup::discover_and_register`]), and the
//! result must stay after an Evolution restart. This module keeps the result.
//! Thus the implementation agrees with `eds-sys/tests/oauth2.rs`: ordinary
//! Rust reads a stored value. A `const gchar *` that an interface vfunc
//! *computes* would cause per-call ownership questions; a stored value does not.
//!
//! ## A custom extension, not a new field on an existing one
//!
//! `ESource` extensions are found by name. `e_source_get_extension` examines
//! all registered subclasses of `ESourceExtension` and compares
//! `ESourceExtensionClass::name` with the requested string. [`crate::account`]
//! finds `[Collection]`, `[Authentication]` and `[Security]` with the same
//! mechanism, and third-party types can use it too. (This is from upstream's
//! `e-source.c`, not an assumption: the lookup walks `g_type_children` of
//! `ESourceExtension` and hashes the `name` field of each concrete subclass. It
//! does not make a distinction between EDS's own subclasses and other
//! subclasses.) Thus the registration of [`Extension`] is all of the "storage
//! this project defines", and no EDS extension changes. As with EDS's
//! extensions, the type must be registered (referenced) before the first
//! lookup for its name. [`read`] and [`apply`] do this the same way as
//! [`crate::account::apply`] does for EDS's own extension types.
//!
//! ## Properties, because properties persist
//!
//! `ESource`'s own (de)serialisation reads and writes only GObject properties
//! with the flag `E_SOURCE_PARAM_SETTING`. That is the only path from a value
//! here to the `.source` file on disk. Thus [`Extension`] installs seven string
//! properties with that flag. Plain Rust state would be lost at a restart.
//!
//! [`apply`] and [`read`] are this module's own interface to the properties.
//! [`crate::account`]'s fields use extensions that EDS defines, which have
//! typed setters. No such accessor exists here, so this module supplies two
//! interfaces to one storage: one for its own callers, and `get_property`/
//! `set_property` for EDS's serialisation.

use std::ffi::{CStr, CString, c_char};
use std::sync::{Mutex, PoisonError};
use std::{mem, ptr};

use eds_sys::{
    E_SOURCE_PARAM_SETTING, ESource, ESourceExtension, ESourceExtensionClass,
    e_source_extension_get_type, e_source_get_extension,
};
use glib_sys::GType;
use gobject_sys::{
    G_PARAM_READWRITE, G_TYPE_STRING, GObject, GObjectClass, GParamSpec, GValue,
    g_object_class_install_property, g_object_get_property, g_object_set_property,
    g_param_spec_string, g_value_get_string, g_value_init, g_value_set_string, g_value_unset,
};
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::instance::Slot;
use jmap_backend_core::marshal::extension_if_present;
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_backend_core::trampoline::{guard, log_critical};

/// The keyfile group name of this storage. It is the `extension_name` argument
/// of `e_source_get_extension`, and the group header in the `.source` file of
/// an account.
///
/// On an EDS without its own `ESourceOAuth2Client`, the group is
/// `[JMAP OAuth2]` and its type is [`Extension`] below. On EDS 3.63.1 and
/// newer, it is upstream's own `[OAuth2 Client]`
/// (`E_SOURCE_EXTENSION_OAUTH2_CLIENT`). Its eight string properties are the
/// seven of this module plus an `issuer` that this module does not write.
/// Upstream's `EOAuth2ServiceDynamic` reads it directly. This module registers
/// no type for that name: with two `ESourceExtension` classes for one name,
/// `e_source_get_extension` returns the one that
/// `source_find_extension_classes_rec` hashed last.
///
/// Accounts configured with an older EDS are not migrated.
/// [`crate::oauth2_service::register`] records that decision.
#[cfg(not(eds_oauth2_dynamic))]
pub const EXTENSION_NAME: &CStr = c"JMAP OAuth2";

/// The `#[cfg(not(eds_oauth2_dynamic))]` arm above has the documentation of
/// this constant. The literal is upstream's `E_SOURCE_EXTENSION_OAUTH2_CLIENT`.
/// That is a `#define` of a string, not a symbol, so bindgen cannot supply it
/// and it is written again here.
#[cfg(eds_oauth2_dynamic)]
pub const EXTENSION_NAME: &CStr = c"OAuth2 Client";

/// The per-account data that the `EOAuth2Service` implementation
/// ([`crate::oauth2_service`]) needs: the RFC 8414 endpoints, the RFC 7591
/// client id and secret, the registered redirect URI, the scope and the
/// resource. Discovery and registration write all seven together. Thus a
/// reader sees no partial state, only "not done yet", where all fields are absent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// RFC 7591 §3.2.1's `client_id`. Absent until registration succeeds.
    pub client_id: Option<String>,
    /// RFC 7591 §3.2.1's `client_secret`. Present only if the server issued one,
    /// although this client registers as public
    /// (`jmap_client::oauth::CLIENT_AUTH_METHOD`).
    pub client_secret: Option<String>,
    /// RFC 8414 §2's `authorization_endpoint`. This is the value of
    /// `EOAuth2Service::get_authentication_uri`.
    pub authorization_endpoint: Option<String>,
    /// RFC 8414 §2's `token_endpoint`. The code is redeemed here, and it is also
    /// the value of `EOAuth2Service::get_refresh_uri`: RFC 6749 uses one endpoint
    /// for the two URIs that EDS asks for.
    pub token_endpoint: Option<String>,
    /// RFC 6749 §3.1.2's `redirect_uri`. Registration fixes it, and each
    /// authentication asks for it again (`get_redirect_uri`).
    pub redirect_uri: Option<String>,
    /// The space-joined RFC 6749 §3.3 scope of the registration. Each
    /// authorization request must name it explicitly. The server decides if it
    /// uses the registered default, and a live server rejected that
    /// (`error=invalid_scope`, Fastmail 2026-08-23). Absent for a deployment
    /// that advertises no scopes.
    pub scope: Option<String>,
    /// The RFC 8707 `resource` indicator: the JMAP session resource of the
    /// token. The authorization request and both token grants name it. Some
    /// deployments require it and reject a request without it
    /// (`error=invalid_target`, Fastmail 2026-08-23). RFC 8707 makes it always
    /// valid to send. Absent when the probe at discovery could not find it.
    pub resource: Option<String>,
}

/// The instance struct: [`ESourceExtension`]'s own state plus one [`Slot`] for
/// the seven values, in the layout that [`ObjectSubclass`] requires.
#[repr(C)]
pub struct Extension {
    parent: ESourceExtension,
    fields: Slot<Mutex<Fields>>,
}

/// The class struct: only [`ESourceExtensionClass`]'s own state. `class_init`
/// writes its leading `name` field, and through *that*, `GObjectClass`'s
/// `get_property`/`set_property`.
#[repr(C)]
pub struct ExtensionClass {
    parent_class: ESourceExtensionClass,
}

/// The seven values, owned as `CString`s. Thus `get_property` gives a borrowed
/// pointer directly to `g_value_set_string` (which copies it), with no
/// allocation on each read. Also [`borrowed`] can give a pointer to the caller
/// of an `EOAuth2Service` vfunc. [`retired`](Self::retired) exists for that case.
#[derive(Default)]
struct Fields {
    client_id: Option<CString>,
    client_secret: Option<CString>,
    authorization_endpoint: Option<CString>,
    token_endpoint: Option<CString>,
    redirect_uri: Option<CString>,
    scope: Option<CString>,
    resource: Option<CString>,
    /// Values that a later write replaced. They stay allocated and are not freed.
    ///
    /// [`borrowed`] gives the caller of an `EOAuth2Service` vfunc a
    /// `const gchar *` into one of the seven fields above. That caller cannot
    /// tell this module when it no longer uses the pointer. EDS's own users of
    /// those vfuncs (`e-oauth2-service.c`) copy the string a few instructions
    /// later, with no lock held and no callback that could free it. Thus this
    /// storage can only promise the lifetime in [`borrowed`]'s doc: valid while
    /// the extension is alive. If a replaced value were freed, a pointer already
    /// in use would become invalid. Thus replaced values move here and are
    /// dropped with the extension, in [`ObjectSubclass::finalize`].
    ///
    /// The growth is limited to the number of writes that *change* a field
    /// ([`Self::set`] does not write an unchanged value). Each such write is a
    /// real change of the discovered OAuth 2.0 configuration of an account. That
    /// occurs at human speed, and EDS's own `source_set_property_from_key_file`
    /// does not write a value that did not change.
    ///
    /// A retired `client_secret` is deliberately not wiped, unlike a PKCE
    /// verifier (`oauth2_service::Verifiers`). The current secret is an ordinary
    /// extension property, so it is already in plain text in the `.source` key
    /// file of the account and in each `GValue` that EDS copies it into. Thus a
    /// wipe of an old copy here gives no protection. Also, the public clients
    /// that this project registers usually have no secret.
    retired: Vec<CString>,
}

impl Fields {
    /// Writes one field by property id. Returns `false` for an id that this
    /// class did not install.
    ///
    /// Never frees the value that it replaces. An unchanged write does nothing,
    /// so the existing allocation (and all pointers from [`borrowed`] into it)
    /// stays. A changed write moves the old value to [`Self::retired`]. These
    /// two rules make [`borrowed`]'s documented lifetime true.
    fn set(&mut self, id: u32, value: Option<CString>) -> bool {
        let replaced = {
            let slot = match id {
                PROP_CLIENT_ID => &mut self.client_id,
                PROP_CLIENT_SECRET => &mut self.client_secret,
                PROP_AUTHORIZATION_ENDPOINT => &mut self.authorization_endpoint,
                PROP_TOKEN_ENDPOINT => &mut self.token_endpoint,
                PROP_REDIRECT_URI => &mut self.redirect_uri,
                PROP_SCOPE => &mut self.scope,
                PROP_RESOURCE => &mut self.resource,
                _ => return false,
            };
            if *slot == value {
                return true;
            }
            mem::replace(slot, value)
        };
        // A move of a `CString` moves the pointer, not the bytes. Thus a pointer
        // into `replaced` stays valid after this push and after all later
        // reallocations of the vector.
        self.retired.extend(replaced);
        true
    }

    /// Returns one field by property id. `None` for an id that this class did
    /// not install, which is different from an installed field without a value.
    fn get(&self, id: u32) -> Option<&Option<CString>> {
        match id {
            PROP_CLIENT_ID => Some(&self.client_id),
            PROP_CLIENT_SECRET => Some(&self.client_secret),
            PROP_AUTHORIZATION_ENDPOINT => Some(&self.authorization_endpoint),
            PROP_TOKEN_ENDPOINT => Some(&self.token_endpoint),
            PROP_REDIRECT_URI => Some(&self.redirect_uri),
            PROP_SCOPE => Some(&self.scope),
            PROP_RESOURCE => Some(&self.resource),
            _ => None,
        }
    }
}

/// The GObject property names that `class_init` installs, in declaration order.
/// A caller that binds or copies this extension between sources must use all
/// of them. `jmap_backend_collection::child_added` binds exactly these from an
/// account to its children.
pub const PROPERTY_NAMES: [&CStr; 7] = [
    c"client-id",
    c"client-secret",
    c"authorization-endpoint",
    c"token-endpoint",
    c"redirect-uri",
    c"scope",
    c"resource",
];

// The ids that `class_init` installs the names with. They are dense from 1,
// because GObject uses 0 for "no property". They apply to this class only.
const PROPERTIES: [(u32, &CStr); 7] = [
    (1, PROPERTY_NAMES[0]),
    (2, PROPERTY_NAMES[1]),
    (3, PROPERTY_NAMES[2]),
    (4, PROPERTY_NAMES[3]),
    (5, PROPERTY_NAMES[4]),
    (6, PROPERTY_NAMES[5]),
    (7, PROPERTY_NAMES[6]),
];

const PROP_CLIENT_ID: u32 = PROPERTIES[0].0;
const PROP_CLIENT_SECRET: u32 = PROPERTIES[1].0;
const PROP_AUTHORIZATION_ENDPOINT: u32 = PROPERTIES[2].0;
const PROP_TOKEN_ENDPOINT: u32 = PROPERTIES[3].0;
const PROP_REDIRECT_URI: u32 = PROPERTIES[4].0;
const PROP_SCOPE: u32 = PROPERTIES[5].0;
const PROP_RESOURCE: u32 = PROPERTIES[6].0;

// SAFETY: both structs are #[repr(C)] and start with ESourceExtension's own
// instance/class structs. eds-sys's tests/layout.rs checks their layout with
// `g_type_query`. ESourceExtension derives from GObject.
unsafe impl ObjectSubclass for Extension {
    const NAME: &'static CStr = c"JmapOAuth2Extension";
    type Instance = Extension;
    type Class = ExtensionClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_source_extension_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` starts with `ESourceExtensionClass`, as the contract
        // above makes sure. `source_find_extension_classes_rec` reads this same
        // field to match an extension name to a `GType`.
        unsafe { (*class).parent_class.name = EXTENSION_NAME.as_ptr() };

        // SAFETY: `ExtensionClass` starts with `ESourceExtensionClass`, which
        // starts with `GObjectClass`. `jmap_mail::settings` uses the same
        // transitive leading-field cast for `CamelJmapSettingsClass`.
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
        // SAFETY: the instance is newly zeroed, which is the empty state of the slot.
        unsafe { (*instance).fields.init(Mutex::new(Fields::default())) };
    }

    unsafe fn finalize(instance: *mut Self::Instance) {
        // SAFETY: the last reference is released, so nothing else can reach `fields`.
        unsafe { (*instance).fields.clear() };
    }
}

/// Writes a property into the storage of the instance.
///
/// Guarded like all other functions that C calls in this crate. This runs in
/// `g_object_set`, which EDS's own `.source` parser calls when it restores an
/// account. A panic that unwinds out of it is undefined behaviour in a process
/// that holds the mail of other persons.
unsafe extern "C" fn set_property(
    object: *mut GObject,
    id: u32,
    value: *mut GValue,
    _pspec: *mut GParamSpec,
) {
    guard("JmapOAuth2Extension::set_property", (), || {
        // SAFETY: GObject passes an instance of this type. `instance_init` runs
        // before a vfunc can be dispatched on it.
        let fields = unsafe { &*object.cast::<Extension>() }
            .fields
            .get()
            .expect("set_property dispatched before instance_init ran");
        let mut fields = fields.lock().unwrap_or_else(PoisonError::into_inner);
        // SAFETY: `value` holds the string type that all pspecs above declare.
        let text = unsafe { read_cstring(value) };
        if !fields.set(id, text) {
            log_critical(&format!(
                "JmapOAuth2Extension has no property with id {id}; the value is dropped"
            ));
        }
    });
}

/// Reads a property. This is the opposite of [`set_property`].
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
        let Some(text) = fields.get(id).map(Option::as_deref) else {
            log_critical(&format!(
                "JmapOAuth2Extension has no property with id {id}; nothing is read"
            ));
            return;
        };
        // SAFETY: `value` is the `GValue` that GObject fills for this get.
        // `g_value_set_string` copies the data at `text` before this call
        // returns, so the borrow on `fields` does not need to live longer.
        unsafe { g_value_set_string(value, text.map_or(ptr::null(), CStr::as_ptr)) };
    });
}

/// Copies the string of a `GValue`, or returns `None` for NULL. A caller clears a
/// field with a NULL property value, as with all string setters in
/// [`crate::account`].
///
/// # Safety
///
/// `value` holds a `G_TYPE_STRING` `GValue`, valid for the call.
unsafe fn read_cstring(value: *const GValue) -> Option<CString> {
    // SAFETY: the caller's contract applies. The returned pointer is borrowed
    // for this call only, so the code below copies it and does not keep it.
    let text = unsafe { g_value_get_string(value) };
    if text.is_null() {
        return None;
    }
    // SAFETY: a non-NULL result is a NUL-terminated string, valid for this call.
    Some(unsafe { CStr::from_ptr(text) }.to_owned())
}

/// Registers [`Extension`] (or finds the existing registration). Only then can
/// `e_source_get_extension`/`e_source_has_extension` match `EXTENSION_NAME` to
/// it. The lookup cannot see an unregistered type, the same as an unregistered
/// EDS extension type (see `e-source.c`'s `source_find_extension_classes_rec`).
///
/// [`apply`] and [`read`] call this before they use a source. Thus their
/// callers do not need to call it, in the same way that a caller of
/// `account::apply` does not register `ESourceCollection`'s `GType` manually.
/// The code that loads this project's EDS module *must* call it. EDS's own
/// `.source` parser restores the extension of each group by the same name
/// lookup. Thus this function must run one time, at module load with the
/// registration of the other types of the module. It must run before EDS
/// parses a keyfile with `[JMAP OAuth2]`. If not, EDS silently ignores that
/// group and does not restore it.
#[cfg(not(eds_oauth2_dynamic))]
pub fn ensure_registered() {
    jmap_backend_core::subclass::register_static::<Extension>();
}

/// If EDS has its own `ESourceOAuth2Client`, "registration" references
/// upstream's type and does not define a type. The name lookup of
/// `e_source_get_extension` walks `g_type_children` of `ESourceExtension`, and a
/// type is not in that list until its `get_type` is called. The
/// `#[cfg(not(eds_oauth2_dynamic))]` arm above has the rest of the contract,
/// which is the same.
#[cfg(eds_oauth2_dynamic)]
pub fn ensure_registered() {
    // SAFETY: no arguments, and the type registers itself on the first call.
    unsafe { eds_sys::e_source_oauth2_client_get_type() };
}

/// Returns the `[JMAP OAuth2]` extension of `source`. It registers the type and
/// creates the group if necessary. This is `e_source_get_extension`'s own
/// "create if absent" rule, which [`crate::account::apply`] also uses for EDS's
/// extensions.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
unsafe fn extension_of(source: *mut ESource) -> *mut Extension {
    ensure_registered();
    // SAFETY: `source` is valid by this function's contract, and the line above
    // registers the type.
    unsafe { e_source_get_extension(source, EXTENSION_NAME.as_ptr()) }.cast()
}

/// Writes `config` to the `[JMAP OAuth2]` extension of `source`, and creates
/// the extension if necessary. All fields are written: `None` clears the old
/// value. [`crate::account::apply`] uses the same idempotent-rewrite rule for
/// the same reason: this runs again each time discovery or registration runs
/// again.
///
/// Because of that rule, a field that this rewrite does not change keeps its
/// allocation, so pointers from `borrowed` into it stay valid. All writes go
/// through `Fields::set`.
///
/// The writes go through `g_object_set_property`, not directly into the
/// fields, so each write emits `notify`. `notify` sends a new discovery or
/// registration through the `GBinding`s that
/// `jmap_backend_collection::child_added` puts between the extension of an
/// account and the extensions of its children. A write outside the property
/// system would update this source, but all children would silently keep
/// the old registration.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn apply(source: *mut ESource, config: &Config) {
    // SAFETY: `source` is valid by this function's contract.
    let extension = unsafe { extension_of(source) };
    for (name, value) in [
        (PROPERTY_NAMES[0], &config.client_id),
        (PROPERTY_NAMES[1], &config.client_secret),
        (PROPERTY_NAMES[2], &config.authorization_endpoint),
        (PROPERTY_NAMES[3], &config.token_endpoint),
        (PROPERTY_NAMES[4], &config.redirect_uri),
        (PROPERTY_NAMES[5], &config.scope),
        (PROPERTY_NAMES[6], &config.resource),
    ] {
        let value_c = value.as_deref().map(cstring_lossy);
        // SAFETY: a live extension instance of this class, which installs all
        // properties in `PROPERTY_NAMES` as strings. The GValue is initialised,
        // filled (NULL clears the field, as `set_property` documents), and
        // unset before the CString that it borrows can drop.
        unsafe {
            let mut gvalue: GValue = std::mem::zeroed();
            g_value_init(&mut gvalue, G_TYPE_STRING);
            g_value_set_string(
                &mut gvalue,
                value_c.as_deref().map_or(ptr::null(), CStr::as_ptr),
            );
            g_object_set_property(extension.cast(), name.as_ptr(), &gvalue);
            g_value_unset(&mut gvalue);
        }
    }
}

/// Returns the current config in the `[JMAP OAuth2]` extension of `source`.
/// All fields are absent if nothing wrote to the source yet, because a read
/// must not create the extension that [`apply`] did not create.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn read(source: *mut ESource) -> Config {
    ensure_registered();

    // SAFETY: `source` is valid by this function's contract, and the class for
    // `EXTENSION_NAME` is a `GObject`.
    let Some(extension) = (unsafe { extension_if_present::<GObject>(source, EXTENSION_NAME) })
    else {
        return Config::default();
    };
    // SAFETY: a live extension of the class for `EXTENSION_NAME` on this build.
    // Both possible classes install all names in `PROPERTY_NAMES` as readable
    // string properties.
    let read_one = |name: &CStr| unsafe { get_string_property(extension.cast(), name) };

    Config {
        client_id: read_one(PROPERTY_NAMES[0]),
        client_secret: read_one(PROPERTY_NAMES[1]),
        authorization_endpoint: read_one(PROPERTY_NAMES[2]),
        token_endpoint: read_one(PROPERTY_NAMES[3]),
        redirect_uri: read_one(PROPERTY_NAMES[4]),
        scope: read_one(PROPERTY_NAMES[5]),
        resource: read_one(PROPERTY_NAMES[6]),
    }
}

/// Returns a copy of one string property of `object`, or `None` if it has no value.
///
/// This function uses the property system, not this module's storage, so
/// [`read`] is one function on all EDS versions. If upstream owns the class
/// of [`EXTENSION_NAME`], there are no Rust fields. Both classes have
/// `g_object_get_property`. Also, the EDS-3.52 test suite thus tests the same
/// code that the 3.63.1+ build runs, not a parallel implementation.
///
/// One difference comes from upstream and this module cannot change it:
/// `ESourceOAuth2Client`'s setters pass values through `e_util_strdup_strip`.
/// Thus an empty or all-whitespace value reads back as unset there, but
/// [`Extension`] keeps it unchanged. No field of a [`Config`] has a useful
/// blank value (an empty client id or token endpoint is not a registration),
/// so the two classes agree where it is important.
///
/// # Safety
///
/// `object` must be a valid `GObject` carrying a readable `G_TYPE_STRING`
/// property called `name`.
unsafe fn get_string_property(object: *mut GObject, name: &CStr) -> Option<String> {
    // SAFETY: the contract above applies. The GValue is initialised before it
    // is filled and unset before it goes out of scope. `read_cstring` copies
    // the data that it points to and does not keep the borrow.
    unsafe {
        let mut gvalue: GValue = std::mem::zeroed();
        g_value_init(&mut gvalue, G_TYPE_STRING);
        g_object_get_property(object, name.as_ptr(), &mut gvalue);
        let text = read_cstring(&gvalue).map(|text| text.to_string_lossy().into_owned());
        g_value_unset(&mut gvalue);
        text
    }
}

// ---------------------------------------------------------------------------
// Borrowed access for `EOAuth2Service`'s vfuncs.

/// Returns one field of the `[JMAP OAuth2]` extension of `source`, borrowed and
/// not copied: a `const gchar *` into the storage of the extension. NULL if
/// nothing wrote to the source (or to that field) yet.
///
/// This shape is different from [`read`] on purpose. `EOAuth2Service`'s
/// vfuncs (`get_client_id` and the four related vfuncs) return
/// `const gchar *`. [`read`]'s owned `String`s are dropped when the vfunc
/// returns, so the caller would get a dangling pointer. This function returns
/// a pointer into the `CString` that [`Fields`] owns. It stays valid while the
/// extension is alive, thus while the `ESource` of the account is alive. That
/// is much longer than one vfunc dispatch.
///
/// ## Why the lock does not keep the pointer valid
///
/// This function releases the mutex below before it returns. Thus the mutex
/// orders the readers and writers of this module, but it cannot protect the
/// caller's *use* of the pointer. The lock cannot be held until then, because a
/// vfunc that returns `const gchar *` has no "done with it" callback. EDS's own
/// callers (`e-oauth2-service.c`) copy the string a few instructions later,
/// into a form table or the URI of a `SoupMessage`, and take no lock of this
/// module. The pointer is safe because nothing frees a value that this
/// function returned: [`Fields::set`] does not write an unchanged value, and
/// moves a changed value into [`Fields::retired`](Fields#structfield.retired).
/// That is dropped only when the extension is finalized.
///
/// EDS's own `EOAuth2Service` implementations use the same rule for these
/// five vfuncs (from `e-oauth2-service-google.c`, not an assumption).
/// `eos_google_get_client_id` returns a `static gchar glob_buff[128]` or a
/// value that `eos_google_read_settings` caches with `g_object_set_data_full`
/// behind an `if (!value)` guard. That value is written one time and is not
/// replaced or freed while the service is alive. EDS's plain extension
/// accessors (`e_source_authentication_get_host` and the others) use a
/// *different* contract. They also have no lock, but each has a `dup_` variant
/// that takes `e_source_extension_property_lock`. A vfunc with a fixed
/// `const gchar *` signature cannot have such an alternative.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
unsafe fn borrowed(
    source: *mut ESource,
    field: impl FnOnce(&Fields) -> &Option<CString>,
) -> *const c_char {
    // If EDS has its own class for this group, the object for `EXTENSION_NAME`
    // is an `ESourceOAuth2Client`, not an [`Extension`]. Then there are no
    // `Fields` to borrow, and the cast below would read the memory of one type
    // as a different type. No caller asks on such a build: the
    // `EOAuth2Service` that uses these vfuncs is not registered
    // (`crate::oauth2_service::register`). Upstream's `EOAuth2ServiceDynamic`
    // controls the account and reads the group through its own extension.
    // A NULL return makes this true by construction, not by inspection.
    if jmap_backend_core::oauth2::EDS_HAS_DYNAMIC_OAUTH2 {
        return ptr::null();
    }

    ensure_registered();

    // SAFETY: `source` is valid by this function's contract. On a build without
    // the dynamic service, `EXTENSION_NAME` is this module's `Extension`. A
    // present extension belongs to the source, and its `instance_init` ran.
    let Some(extension) = (unsafe { extension_if_present::<Extension>(source, EXTENSION_NAME) })
    else {
        return ptr::null();
    };
    let fields = unsafe { &*extension }
        .fields
        .get()
        .expect("e_source_get_extension returned an instance instance_init did not run on");
    let fields = fields.lock().unwrap_or_else(PoisonError::into_inner);
    field(&fields).as_deref().map_or(ptr::null(), CStr::as_ptr)
}

/// Returns the stored RFC 7591 `client_id` of `source`, or NULL before
/// registration. See `borrowed` for the lifetime of the pointer.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn client_id(source: *mut ESource) -> *const c_char {
    unsafe { borrowed(source, |fields| &fields.client_id) }
}

/// Returns the stored RFC 7591 `client_secret` of `source`, or NULL. NULL for
/// the usual public client, and before registration. See `borrowed` for the
/// lifetime of the pointer.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn client_secret(source: *mut ESource) -> *const c_char {
    unsafe { borrowed(source, |fields| &fields.client_secret) }
}

/// Returns the stored RFC 8414 `authorization_endpoint` of `source`, or NULL
/// before discovery. See `borrowed` for the lifetime of the pointer.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn authorization_endpoint(source: *mut ESource) -> *const c_char {
    unsafe { borrowed(source, |fields| &fields.authorization_endpoint) }
}

/// Returns the stored RFC 8414 `token_endpoint` of `source`, or NULL before
/// discovery. See `borrowed` for the lifetime of the pointer.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn token_endpoint(source: *mut ESource) -> *const c_char {
    unsafe { borrowed(source, |fields| &fields.token_endpoint) }
}

/// Returns the registered `redirect_uri` of `source`, or NULL before
/// registration. See `borrowed` for the lifetime of the pointer.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn redirect_uri(source: *mut ESource) -> *const c_char {
    unsafe { borrowed(source, |fields| &fields.redirect_uri) }
}

/// Returns the registered `scope` of `source`, or NULL for a deployment that
/// advertises no scopes. See `borrowed` for the lifetime of the pointer.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn scope(source: *mut ESource) -> *const c_char {
    unsafe { borrowed(source, |fields| &fields.scope) }
}

/// Returns the stored RFC 8707 `resource` indicator of `source`, or NULL if
/// discovery could not find one. See `borrowed` for the lifetime of the pointer.
///
/// # Safety
///
/// `source` must be a valid `ESource`.
pub unsafe fn resource(source: *mut ESource) -> *const c_char {
    unsafe { borrowed(source, |fields| &fields.resource) }
}
