// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelJmapSettings`: where a JMAP account's server lives on the Camel side.
//!
//! A `CamelService` — which a store is — is configured through a
//! `CamelSettings` object, and which class that object has is a property of
//! the service's *class*: `CamelServiceClass.settings_type`. Inherited from
//! `CamelOfflineStore` it is `CamelOfflineSettings`, which knows about offline
//! synchronisation and nothing about a network. Host, port, user name and
//! security method live on the `CamelNetworkSettings` interface instead, and
//! no stock Camel settings class implements it — every provider in the EDS
//! tree, IMAPx and POP and SMTP alike, declares a settings subclass of its own
//! that does. This is the JMAP one.
//!
//! Implementing that interface is two halves, and only the first is visible in
//! the type system:
//!
//! 1. *Claiming* it, which [`ObjectSubclass::interfaces`] does, is what makes
//!    `CAMEL_IS_NETWORK_SETTINGS` true and what lets Camel's accessors —
//!    `camel_network_settings_get_host` and friends — be called on the object
//!    at all.
//! 2. *Overriding its properties*, which [`JmapSettings::class_init`] does, is
//!    what makes those same values reachable by name through `g_object_set`.
//!
//! Skipping the second half is the interesting failure, because it is nearly
//! silent: the type still passes every `CAMEL_IS_NETWORK_SETTINGS` check, the
//! accessors still work (the interface keeps its values in per-object data,
//! not in a struct field), and the only complaint is five GLib criticals at
//! class-init time. What breaks is everything that goes *through* the property
//! system — which on this path is everything that matters.
//! `e_source_camel_configure_service` binds an `ESource`'s extension
//! properties to the settings object's by name, so an account configured by
//! EDS would end up with no host; `camel_settings_clone` and
//! `camel_settings_equal` walk the property list, so two accounts on different
//! servers would compare equal.
//!
//! [`ObjectSubclass::interfaces`]: jmap_backend_core::subclass::ObjectSubclass::interfaces
//!
//! The overrides forward to the interface's own accessors rather than to
//! storage of their own, which is what Camel's providers do and what keeps the
//! two doors — property and accessor — looking at one value instead of two.

use std::ffi::CStr;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use eds_sys::{
    CamelNetworkSecurityMethod, CamelNetworkSettings, CamelOfflineSettings,
    CamelOfflineSettingsClass, CamelSettings, camel_network_settings_dup_auth_mechanism,
    camel_network_settings_dup_host, camel_network_settings_dup_user,
    camel_network_settings_get_port, camel_network_settings_get_security_method,
    camel_network_settings_get_type, camel_network_settings_set_auth_mechanism,
    camel_network_settings_set_host, camel_network_settings_set_port,
    camel_network_settings_set_security_method, camel_network_settings_set_user,
    camel_offline_settings_get_type,
};
use glib_sys::{GFALSE, GType};
use gobject_sys::{
    G_PARAM_READWRITE, GObject, GObjectClass, GParamSpec, GValue, g_object_class_install_property,
    g_object_class_override_property, g_param_spec_boolean, g_value_get_boolean, g_value_get_enum,
    g_value_get_string, g_value_get_uint, g_value_set_boolean, g_value_set_enum, g_value_set_uint,
    g_value_take_string,
};
use jmap_backend_core::subclass::{InterfaceDecl, ObjectSubclass, register_static};
use jmap_backend_core::trampoline::{guard, log_critical};

/// The instance struct. Every value of the `CamelNetworkSettings` interface
/// is stored by the interface itself, in per-object data. `rebase_urls` is
/// the one value this type stores on its own: the rebase opt-in has no
/// interface to back it, so `jmap-backend-collection`'s `mail_child` module
/// binds it here by plain property name instead.
#[repr(C)]
pub struct JmapSettings {
    parent: CamelOfflineSettings,
    rebase_urls: AtomicBool,
}

/// The class struct, where the property overrides and the two accessors that
/// service them are installed.
#[repr(C)]
pub struct JmapSettingsClass {
    parent_class: CamelOfflineSettingsClass,
}

/// The property IDs the overrides are installed under.
///
/// Local to this class and dense from 1 — GObject treats 0 as "no property" —
/// rather than shared with anything. A property the *parent* installed never
/// reaches the accessors below: `g_object_set_property` dispatches to the
/// class that owns the pspec, so `filter-inbox` goes to
/// `CamelStoreSettings`'s own `set_property` and these five IDs cannot collide
/// with it.
///
/// The names are the ones `CamelNetworkSettings` declares. A typo is not a
/// build error — `g_object_class_override_property` on a name no interface
/// declares is a GLib critical and a property that is then never found.
const PROPERTIES: [(u32, &CStr); 5] = [
    (1, c"auth-mechanism"),
    (2, c"host"),
    (3, c"port"),
    (4, c"security-method"),
    (5, c"user"),
];

const PROP_AUTH_MECHANISM: u32 = PROPERTIES[0].0;
const PROP_HOST: u32 = PROPERTIES[1].0;
const PROP_PORT: u32 = PROPERTIES[2].0;
const PROP_SECURITY_METHOD: u32 = PROPERTIES[3].0;
const PROP_USER: u32 = PROPERTIES[4].0;

/// The id of the one property this class declares on its own, rather than
/// overriding from `CamelNetworkSettings`. Dense with [`PROPERTIES`], so it
/// cannot collide with them either.
const PROP_REBASE_URLS: u32 = 6;

/// The name of [`PROP_REBASE_URLS`]: `e_source_camel_generate_subtype`
/// mirrors it, by this name, onto the `ESourceCamel` subtype it generates for
/// the `jmap` protocol. `jmap-backend-collection`'s `mail_child` module binds
/// the account's `[JMAP Rebase]` onto that mirrored property.
const PROP_NAME_REBASE_URLS: &CStr = c"rebase-urls";

// SAFETY: both structs are #[repr(C)] and lead with the CamelOfflineSettings
// instance and class structs, whose layouts eds-sys's tests/layout.rs checks
// against `g_type_query`; CamelOfflineSettings derives from
// CamelStoreSettings, from CamelSettings, from GObject.
unsafe impl ObjectSubclass for JmapSettings {
    /// `CamelJmapSettings`, matching `CamelJmapStore` and Camel's own
    /// `Camel<Protocol>Settings` naming.
    const NAME: &'static CStr = c"CamelJmapSettings";
    type Instance = JmapSettings;
    type Class = JmapSettingsClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { camel_offline_settings_get_type() }
    }

    fn interfaces() -> Vec<InterfaceDecl> {
        // `CamelNetworkSettings` declares no vfuncs — the five properties
        // `class_init` overrides below are the whole of it — so there is no
        // vtable for this type to fill.
        //
        // SAFETY: as above.
        vec![InterfaceDecl::defaults(unsafe {
            camel_network_settings_get_type()
        })]
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: the class leads with the parent class struct, which derives
        // from GObjectClass — the contract above.
        let object_class = class.cast::<GObjectClass>();
        unsafe {
            (*object_class).set_property = Some(set_property);
            (*object_class).get_property = Some(get_property);

            for (id, name) in PROPERTIES {
                // The interface was added to the type before registration
                // handed it back, which is what makes these names findable
                // here.
                g_object_class_override_property(object_class, id, name.as_ptr());
            }

            // Not an interface property: a plain boolean this class stores on
            // its own instance. `FALSE` is also the default of a never-set
            // `[JMAP Rebase]`, so an account with no opt-in gets the same
            // answer on both sides.
            let pspec = g_param_spec_boolean(
                PROP_NAME_REBASE_URLS.as_ptr(),
                ptr::null(),
                ptr::null(),
                GFALSE,
                G_PARAM_READWRITE,
            );
            g_object_class_install_property(object_class, PROP_REBASE_URLS, pspec);
        }
    }
}

/// Registers the settings type, or returns it if it is already registered.
///
/// Statically, for the same reason as [`crate::store::store_type`]: a Camel
/// provider is not a `GTypeModule`, and the provider struct Camel keeps names
/// types that must stay instantiable for the life of the process.
pub fn settings_type() -> GType {
    register_static::<JmapSettings>()
}

/// Routes an overridden interface property to the interface's own setter.
///
/// Guarded like every other function C calls into this crate: this one runs
/// inside `g_object_set`, which Evolution reaches from its account editor and
/// EDS from a property binding, and a panic unwinding out of either is
/// undefined behaviour in a process full of other people's mail.
unsafe extern "C" fn set_property(
    object: *mut GObject,
    id: u32,
    value: *mut GValue,
    _pspec: *mut GParamSpec,
) {
    guard("CamelJmapSettings::set_property", (), || {
        let settings = object.cast::<CamelNetworkSettings>();
        // SAFETY: GObject passes an instance of this type and a GValue holding
        // the type the overridden property declares. The setters copy the
        // string, so the borrow does not outlive the call.
        unsafe {
            match id {
                PROP_AUTH_MECHANISM => {
                    camel_network_settings_set_auth_mechanism(settings, g_value_get_string(value));
                }
                PROP_HOST => camel_network_settings_set_host(settings, g_value_get_string(value)),
                // The property is a `guint` and the accessor takes a
                // `guint16`, so the narrowing is Camel's own and not a choice
                // here; the pspec's range is what keeps it lossless.
                PROP_PORT => {
                    camel_network_settings_set_port(settings, g_value_get_uint(value) as u16)
                }
                PROP_SECURITY_METHOD => camel_network_settings_set_security_method(
                    settings,
                    g_value_get_enum(value) as CamelNetworkSecurityMethod,
                ),
                PROP_USER => camel_network_settings_set_user(settings, g_value_get_string(value)),
                // Not an interface property: read straight from the GValue
                // and store it on this instance, by the contract above.
                PROP_REBASE_URLS => object
                    .cast::<JmapSettings>()
                    .as_ref()
                    .expect("object is a live instance by the contract above")
                    .rebase_urls
                    .store(g_value_get_boolean(value) != GFALSE, Ordering::Relaxed),
                _ => log_critical(&format!(
                    "CamelJmapSettings has no property with id {id}; the value is dropped"
                )),
            }
        }
    });
}

/// The reading half. The string properties use the `dup_` accessors and
/// `g_value_take_string`, so the `GValue` takes the copy Camel just made
/// rather than pointing into storage another thread may be about to replace.
unsafe extern "C" fn get_property(
    object: *mut GObject,
    id: u32,
    value: *mut GValue,
    _pspec: *mut GParamSpec,
) {
    guard("CamelJmapSettings::get_property", (), || {
        let settings = object.cast::<CamelNetworkSettings>();
        // SAFETY: as in `set_property`; the `dup_` accessors return a
        // g_malloc'd string whose ownership `g_value_take_string` accepts.
        unsafe {
            match id {
                PROP_AUTH_MECHANISM => {
                    g_value_take_string(value, camel_network_settings_dup_auth_mechanism(settings))
                }
                PROP_HOST => {
                    g_value_take_string(value, camel_network_settings_dup_host(settings));
                }
                PROP_PORT => {
                    g_value_set_uint(value, camel_network_settings_get_port(settings).into());
                }
                PROP_SECURITY_METHOD => g_value_set_enum(
                    value,
                    camel_network_settings_get_security_method(settings) as i32,
                ),
                PROP_USER => {
                    g_value_take_string(value, camel_network_settings_dup_user(settings));
                }
                PROP_REBASE_URLS => g_value_set_boolean(
                    value,
                    object
                        .cast::<JmapSettings>()
                        .as_ref()
                        .expect("object is a live instance by the contract above")
                        .rebase_urls
                        .load(Ordering::Relaxed) as _,
                ),
                _ => log_critical(&format!(
                    "CamelJmapSettings has no property with id {id}; nothing is read"
                )),
            }
        }
    });
}

/// Whether `settings` opts in to the rebase of the JMAP session's URLs onto
/// the origin of the actual connection (see
/// [`jmap_backend_core::rebase::rebase_urls`], the account-side read of the
/// same fact).
///
/// `jmap-backend-collection`'s `mail_child` module binds this property from
/// the account's `[JMAP Rebase]` onto the mail child's own settings
/// extension, which `e_source_camel_configure_service` then shares with the
/// live `CamelService` as its `CamelSettings`. This function is the mail
/// side's read of that value: a `CamelService` has no `ESource` of its own
/// to read the opt-in from directly (see `crate::connect`'s module doc).
///
/// # Safety
///
/// `settings` must be a valid instance of the type [`settings_type`]
/// registers, i.e. the one `camel_service_ref_settings` returns for a JMAP
/// store or transport. It is only read from, and nothing outlives the call.
pub unsafe fn rebase_urls(settings: *mut CamelSettings) -> bool {
    // SAFETY: the contract above.
    unsafe {
        settings
            .cast::<JmapSettings>()
            .as_ref()
            .expect("settings is a live instance by the contract above")
            .rebase_urls
            .load(Ordering::Relaxed)
    }
}
