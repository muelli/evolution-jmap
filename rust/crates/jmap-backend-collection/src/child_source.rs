// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Writes a child source: the settings from `jmap-collection-sync`, onto the
//! `ESource` that `e_collection_backend_new_child` returns.
//!
//! This is the mirror of [`crate::resource_id`], and it happens first. The
//! populate that discovers a collection writes its child once. Every later
//! start reads the child back.
//! [`Child::settings`](jmap_collection_sync::Child::settings) already gives
//! *what* to write, as `(group, key, value)` triples in `.source` keyfile
//! spelling. This module needs the headers, because an `ESource` is not a
//! keyfile: the group is an extension object, the key is a GObject property,
//! and the value has a type.
//!
//! ## Every setting is written, or the child is not trusted
//!
//! [`apply`] refuses a setting that has no known property; it does not skip
//! it. A skip is a much worse failure. The child still exists and looks like
//! an address book of this account, but one necessary property is missing.
//! Without `[Resource] Identity`, EDS **deletes** the cache file of the child
//! on the next start ([`crate::resource_id`]). Without `[Authentication] Host`,
//! the address book backend sends every request to no server.
//!
//! The settings are a closed set. Thus an [`UnwritableSetting`] means that
//! `jmap-collection-sync` has a new setting that this module cannot write.
//! `tests/child_source.rs` writes every shape of child for every shape of
//! account, so the result is a failed test, not a broken account.
//!
//! ## Here, `e_source_get_extension` is *wanted* for what it does
//!
//! The two readers in this crate do not call it on a source they get, because
//! it creates a missing extension. In this module, the creation is the
//! purpose: the source is a new child of this backend. `[Address Book]` is what
//! makes it an address book to `collection_backend_child_is_contacts()` and to
//! the factory that loads it. This module must not create the extension of the
//! *other* kind, because either factory can claim a child with both. Thus it
//! gets only the extensions that a setting names.
//!
//! ## The two values that are not strings
//!
//! - `[Authentication] Port` is a `guint16` on the extension and text in the
//!   keyfile. This module parses it, so a failure is
//!   [`UnwritableSetting::WrongType`], not an unset port.
//! - `[Security] Method` is the string of `ESourceSecurity:method` ("tls" or
//!   "none"). The JMAP backends read the derived boolean
//!   `ESourceSecurity:secure`. This module writes through
//!   `e_source_security_set_method()`, not `…_set_secure()`. Thus the spelling
//!   in [`Child::settings`](jmap_collection_sync::Child::settings) must be
//!   correct, and a test finds it if it is not.

use std::ffi::CStr;
use std::fmt;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_CALENDAR, E_SOURCE_EXTENSION_RESOURCE,
    E_SOURCE_EXTENSION_SECURITY, ESource, ESourceAuthentication, ESourceBackend, ESourceResource,
    ESourceSecurity, ESourceSelectable, e_source_address_book_get_type,
    e_source_authentication_get_type, e_source_authentication_set_host,
    e_source_authentication_set_method, e_source_authentication_set_port,
    e_source_authentication_set_user, e_source_backend_set_backend_name,
    e_source_calendar_get_type, e_source_get_extension, e_source_resource_get_type,
    e_source_resource_set_identity, e_source_security_get_type, e_source_security_set_method,
    e_source_selectable_set_color, e_source_set_display_name,
};
use jmap_backend_core::error::cstring_lossy;
use jmap_collection_sync::Setting;
use jmap_collection_sync::child_source::{
    EXTENSION_AUTHENTICATION, EXTENSION_CALENDAR, EXTENSION_DATA_SOURCE, EXTENSION_RESOURCE,
    EXTENSION_SECURITY,
};

use crate::resource_id::KIND_EXTENSIONS;

/// The keyfile groups that are `ESource` extensions but not the kind of a
/// child. Each has the EDS constant for the same group.
///
/// This pairs two spellings of one string, as [`KIND_EXTENSIONS`] does.
/// `tests/child_source.rs` compares them with the EDS `#define`s. The crate
/// that decides the child builds without the EDS headers and has its own
/// literals. If a pair differs, the compiler does not see it: the setting is
/// never written, and nothing reports it.
///
/// [`EXTENSION_DATA_SOURCE`] is deliberately not here. It is the own group of
/// the `ESource`, not an extension. EDS spells it in `e-source.c`, not in a
/// header, so there is no constant for it.
pub const EXTENSIONS: [(&CStr, &str); 3] = [
    (E_SOURCE_EXTENSION_RESOURCE, EXTENSION_RESOURCE),
    (E_SOURCE_EXTENSION_AUTHENTICATION, EXTENSION_AUTHENTICATION),
    (E_SOURCE_EXTENSION_SECURITY, EXTENSION_SECURITY),
];

/// Writes every setting onto `source`, in sequence.
///
/// The settings come from
/// [`Child::settings`](jmap_collection_sync::Child::settings). They make the
/// source from `e_collection_backend_new_child` into an address book or a
/// calendar of this account.
///
/// After an error, the settings before it are written and the remaining ones
/// are not. Thus the caller must abandon the child: this module exists to
/// prevent a half-written child.
///
/// # Safety
///
/// `source` must be a valid `ESource`, a child of the collection of this
/// backend. This call takes no reference to it and does not outlive it.
pub unsafe fn apply(source: *mut ESource, settings: &[Setting]) -> Result<(), UnwritableSetting> {
    // `e_source_get_extension` finds only registered children of
    // `E_TYPE_SOURCE_EXTENSION`. For an unregistered type it creates nothing
    // and returns NULL. A reference to the GType registers it.
    // SAFETY: no arguments, and the type system initialises itself.
    unsafe {
        e_source_address_book_get_type();
        e_source_calendar_get_type();
        e_source_resource_get_type();
        e_source_authentication_get_type();
        e_source_security_get_type();
    }

    for setting in settings {
        // SAFETY: a valid source by the contract of this function.
        unsafe { write(source, setting) }?;
    }
    Ok(())
}

/// Writes one setting onto the property that it names.
///
/// # Safety
///
/// As [`apply`].
unsafe fn write(source: *mut ESource, setting: &Setting) -> Result<(), UnwritableSetting> {
    // Truncate at an interior NUL; do not refuse. The display name is server
    // data, and a JSON string can contain a NUL that a C string cannot. A
    // refusal would also refuse the child. Every C caller reads only the part
    // before the NUL anyway.
    let value = cstring_lossy(&setting.value);

    // The kind of the child, if the group names one. This comes first, because
    // it separates `BackendName` from a group without that property.
    let kind = KIND_EXTENSIONS
        .iter()
        .find(|(_, group)| *group == setting.group)
        .map(|(defined, _)| *defined);

    match (setting.group, setting.key) {
        (EXTENSION_DATA_SOURCE, "DisplayName") => {
            // A property of the source, not of an extension: the name that
            // Evolution shows in the sidebar.
            // SAFETY: a live source, and the setter copies the string.
            unsafe { e_source_set_display_name(source, value.as_ptr()) };
        }
        // The one setting with a variable group, because the group *is* the
        // kind. An address book child has its backend name under
        // `[Address Book]`, and a calendar under `[Calendar]`. Both name the
        // same factory.
        (_, "BackendName") => {
            let name = kind.ok_or_else(|| UnwritableSetting::unknown(setting))?;
            // SAFETY: `name` is a header constant naming an extension deriving
            // from `ESourceBackend`, whose type `apply` registers; the
            // extension is created on demand and owned by the source.
            unsafe {
                let backend: *mut ESourceBackend = extension(source, name);
                e_source_backend_set_backend_name(backend, value.as_ptr());
            }
        }
        (EXTENSION_RESOURCE, "Identity") => {
            // The address book and calendar backends read this field as the
            // JMAP object to fetch. `dup_resource_id` derives the resource id
            // of this child from it. Without it, EDS deletes the cache.
            // SAFETY: as above, with `ESourceResource` the type the name
            // selects.
            unsafe {
                let resource: *mut ESourceResource = extension(source, E_SOURCE_EXTENSION_RESOURCE);
                e_source_resource_set_identity(resource, value.as_ptr());
            }
        }
        (EXTENSION_AUTHENTICATION, key @ ("Host" | "User" | "Method")) => {
            // SAFETY: as above, with `ESourceAuthentication` the type the name
            // selects; each setter copies the string it is given.
            unsafe {
                let auth: *mut ESourceAuthentication =
                    extension(source, E_SOURCE_EXTENSION_AUTHENTICATION);
                match key {
                    "Host" => e_source_authentication_set_host(auth, value.as_ptr()),
                    "User" => e_source_authentication_set_user(auth, value.as_ptr()),
                    _ => e_source_authentication_set_method(auth, value.as_ptr()),
                }
            }
        }
        (EXTENSION_AUTHENTICATION, "Port") => {
            let port: u16 = setting
                .value
                .parse()
                .map_err(|_| UnwritableSetting::wrong_type(setting))?;
            // SAFETY: as above.
            unsafe {
                let auth: *mut ESourceAuthentication =
                    extension(source, E_SOURCE_EXTENSION_AUTHENTICATION);
                e_source_authentication_set_port(auth, port);
            }
        }
        (EXTENSION_SECURITY, "Method") => {
            // The string, not the derived `secure` boolean. See the module
            // comment.
            // SAFETY: as above, with `ESourceSecurity` the type the name
            // selects.
            unsafe {
                let security: *mut ESourceSecurity = extension(source, E_SOURCE_EXTENSION_SECURITY);
                e_source_security_set_method(security, value.as_ptr());
            }
        }
        (EXTENSION_CALENDAR, "Color") => {
            // `ESourceCalendar` derives from `ESourceSelectable`
            // (`e-source-calendar.h`). Thus the same "Calendar" extension
            // object accepts `e_source_selectable_set_color`, and no second
            // extension is necessary.
            // SAFETY: as above, with `ESourceSelectable` the base class from
            // which the type of the "Calendar" extension derives.
            unsafe {
                let selectable: *mut ESourceSelectable =
                    extension(source, E_SOURCE_EXTENSION_CALENDAR);
                e_source_selectable_set_color(selectable, value.as_ptr());
            }
        }
        _ => return Err(UnwritableSetting::unknown(setting)),
    }
    Ok(())
}

/// The extension of `source` with that name, created if it does not exist.
///
/// # Safety
///
/// `source` must be a valid `ESource`, `name` must name an `ESourceExtension`
/// subclass whose type is registered, and `T` must be that subclass. The
/// setters check with `g_return_if_fail`, so a mismatch causes a critical, not
/// undefined behaviour, but the type is still false to the compiler.
unsafe fn extension<T>(source: *mut ESource, name: &CStr) -> *mut T {
    // SAFETY: the contract above; the extension is owned by the source and
    // lives as long as it does.
    unsafe { e_source_get_extension(source, name.as_ptr()) }.cast()
}

/// A setting from which no `ESource` property was written.
///
/// This is not a `SourceError`: those are faults in the account of the *user*,
/// reported to Evolution as `E_CLIENT_ERROR` codes. This is a fault in this
/// backend, because the settings come from `jmap-collection-sync`, not from the
/// user. A caller can only abandon the child and log the setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnwritableSetting {
    /// No property of any extension is known for this group and key.
    UnknownProperty {
        group: &'static str,
        key: &'static str,
    },
    /// The property is known, but it cannot hold the value.
    WrongType {
        group: &'static str,
        key: &'static str,
        value: String,
    },
}

impl UnwritableSetting {
    fn unknown(setting: &Setting) -> Self {
        Self::UnknownProperty {
            group: setting.group,
            key: setting.key,
        }
    }

    fn wrong_type(setting: &Setting) -> Self {
        Self::WrongType {
            group: setting.group,
            key: setting.key,
            value: setting.value.clone(),
        }
    }
}

impl fmt::Display for UnwritableSetting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownProperty { group, key } => {
                write!(f, "no ESource property is known for [{group}] {key}")
            }
            Self::WrongType { group, key, value } => {
                write!(f, "[{group}] {key} cannot be set to \"{value}\"")
            }
        }
    }
}

impl std::error::Error for UnwritableSetting {}

#[cfg(test)]
mod unwritable_setting_display_tests {
    use super::*;

    /// The `Display` of `UnwritableSetting` writes text for every variant.
    /// Without this test, an empty `Display` compiles, and an abandoned child
    /// logs an empty reason.
    #[test]
    fn every_variant_names_its_group_and_key() {
        let unknown = UnwritableSetting::UnknownProperty {
            group: "Mail Account",
            key: "BackendName",
        };
        let message = unknown.to_string();
        assert!(message.contains("Mail Account"));
        assert!(message.contains("BackendName"));

        let wrong_type = UnwritableSetting::WrongType {
            group: "Authentication",
            key: "Port",
            value: "https".to_owned(),
        };
        let message = wrong_type.to_string();
        assert!(message.contains("Authentication"));
        assert!(message.contains("Port"));
        assert!(message.contains("https"));
    }
}
