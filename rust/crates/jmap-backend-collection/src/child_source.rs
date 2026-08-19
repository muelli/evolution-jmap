// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Writing a child source — the settings out of `jmap-collection-sync`, onto
//! the `ESource` `e_collection_backend_new_child` hands back.
//!
//! The mirror of [`crate::resource_id`], and the half that happens first: a
//! child is written once, by the populate that discovers its collection, and
//! read back on every start after that.
//! [`Child::settings`](jmap_collection_sync::Child::settings) already says
//! *what* to write, as `(group, key, value)` triples that are exactly how a
//! `.source` keyfile spells them; this is the part that needs the headers,
//! because an `ESource` is not a keyfile — the group is an extension object,
//! the key is a GObject property, and the value has a type.
//!
//! ## Every setting is written, or none of the child is trusted
//!
//! [`apply`] refuses a setting it has no property for instead of skipping it.
//! Skipping would be the worse failure by far: the child would still be
//! created, would still look like an address book of this account, and would be
//! missing whichever single property makes it work — `[Resource] Identity`,
//! whose absence makes EDS **delete** the child's cache file on the next start
//! ([`crate::resource_id`]), or `[Authentication] Host`, whose absence makes
//! every request the address book backend sends go to no server at all.
//!
//! The settings are a closed set, so an [`UnwritableSetting`] means
//! `jmap-collection-sync` grew a setting this module was not taught to write.
//! `tests/child_source.rs` writes every shape of child against every shape of
//! account, so that is a red test here rather than a broken account there.
//!
//! ## Here `e_source_get_extension` is *wanted* for what it does
//!
//! Both readers in this crate go out of their way not to call it on a source
//! they were handed, because it creates the extension it cannot find. This
//! module is where creating them is the whole point: the source is a fresh
//! child of ours, and giving it `[Address Book]` is precisely what makes it an
//! address book to `collection_backend_child_is_contacts()` and to the factory
//! that will load it. What it must not do is create the extension of the
//! *other* kind — a child carrying both is one either factory may claim — so
//! nothing here reaches for an extension a setting did not name.
//!
//! ## The two values that are not strings
//!
//! - `[Authentication] Port` is a `guint16` on the extension and text in the
//!   keyfile. The parse is this module's, so its failure is
//!   [`UnwritableSetting::WrongType`] rather than a silently unset port.
//! - `[Security] Method` is the string `ESourceSecurity:method` holds — "tls"
//!   or "none" — while the JMAP backends read the derived boolean
//!   `ESourceSecurity:secure`. Written through
//!   `e_source_security_set_method()` rather than `…_set_secure()` so that the
//!   spelling in [`Child::settings`](jmap_collection_sync::Child::settings) is
//!   the spelling that has to be right, and a test can catch it if it is not.

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

/// The keyfile groups that are `ESource` extensions and are not a child's kind,
/// each paired with the constant EDS spells the same group with.
///
/// The same two-spellings-of-one-string pairing as [`KIND_EXTENSIONS`], held
/// against EDS's `#define`s by `tests/child_source.rs` for the same reason: the
/// crate that decides what a child is builds without the EDS headers and so
/// carries its own literals, and a pair that drifts apart is not a compile
/// error — it is a setting that is quietly never written.
///
/// [`EXTENSION_DATA_SOURCE`] is deliberately not here. It is the `ESource`'s
/// own group rather than an extension, and EDS spells it in `e-source.c`
/// instead of in a header, so there is no constant to pair it with.
pub const EXTENSIONS: [(&CStr, &str); 3] = [
    (E_SOURCE_EXTENSION_RESOURCE, EXTENSION_RESOURCE),
    (E_SOURCE_EXTENSION_AUTHENTICATION, EXTENSION_AUTHENTICATION),
    (E_SOURCE_EXTENSION_SECURITY, EXTENSION_SECURITY),
];

/// Writes every setting onto `source`, in order.
///
/// The settings are
/// [`Child::settings`](jmap_collection_sync::Child::settings)' — everything
/// that makes the source `e_collection_backend_new_child` returned into an
/// address book or a calendar of this account.
///
/// An error leaves the settings before it written and the rest not, which is
/// why the caller's answer to one has to be to abandon the child rather than to
/// carry on with it: a half-written child is exactly the shape this module
/// exists to avoid.
///
/// # Safety
///
/// `source` must be a valid `ESource` — a child of this backend's collection,
/// which this call takes no reference to and does not outlive.
pub unsafe fn apply(source: *mut ESource, settings: &[Setting]) -> Result<(), UnwritableSetting> {
    // As everywhere an extension is looked up by name: `e_source_get_extension`
    // walks the registered children of `E_TYPE_SOURCE_EXTENSION`, so a type
    // nothing has referenced yet is one it cannot find — and here it would
    // *create* nothing and return NULL. Referencing the GType registers it.
    // SAFETY: no arguments, and the type system initialises itself.
    unsafe {
        e_source_address_book_get_type();
        e_source_calendar_get_type();
        e_source_resource_get_type();
        e_source_authentication_get_type();
        e_source_security_get_type();
    }

    for setting in settings {
        // SAFETY: a valid source by this function's contract.
        unsafe { write(source, setting) }?;
    }
    Ok(())
}

/// One setting, onto the property it names.
///
/// # Safety
///
/// As [`apply`].
unsafe fn write(source: *mut ESource, setting: &Setting) -> Result<(), UnwritableSetting> {
    // Truncating at an interior NUL rather than refusing: the display name is
    // server data, a JSON string may carry a NUL where a C string may not, and
    // refusing the write would mean refusing the child. What is kept is what
    // the name would have meant to every C caller downstream anyway.
    let value = cstring_lossy(&setting.value);

    // The child's kind, when the group names one. Read first because it is
    // what tells `BackendName` apart from a group that has no such property.
    let kind = KIND_EXTENSIONS
        .iter()
        .find(|(_, group)| *group == setting.group)
        .map(|(defined, _)| *defined);

    match (setting.group, setting.key) {
        (EXTENSION_DATA_SOURCE, "DisplayName") => {
            // The source's own property, not an extension's: what Evolution
            // shows in the sidebar.
            // SAFETY: a live source, and the setter copies the string.
            unsafe { e_source_set_display_name(source, value.as_ptr()) };
        }
        // The one setting whose group varies, because the group *is* the kind:
        // an address book child's backend name lives under `[Address Book]`
        // and a calendar's under `[Calendar]`. Both name the same factory.
        (_, "BackendName") => {
            let name = kind.ok_or_else(|| UnwritableSetting::unknown(setting))?;
            // SAFETY: `name` is a header constant naming an extension deriving
            // from `ESourceBackend`, whose type is registered above; the
            // extension is created on demand and owned by the source.
            unsafe {
                let backend: *mut ESourceBackend = extension(source, name);
                e_source_backend_set_backend_name(backend, value.as_ptr());
            }
        }
        (EXTENSION_RESOURCE, "Identity") => {
            // The field the address book and calendar backends read as the
            // JMAP object to fetch, and the field `dup_resource_id` derives
            // this child's resource id from. Its absence is a deleted cache.
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
            // The string, not the derived `secure` boolean — see the module
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
            // (`e-source-calendar.h`), so the same "Calendar" extension
            // object answers `e_source_selectable_set_color` too — no second
            // extension to look up or register.
            // SAFETY: as above, with `ESourceSelectable` the base class the
            // "Calendar" extension's own type derives from.
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

/// `source`'s extension of that name, created if it has none yet.
///
/// # Safety
///
/// `source` must be a valid `ESource`, `name` must name an `ESourceExtension`
/// subclass whose type is registered, and `T` must be that subclass — the
/// setters below check with `g_return_if_fail`, so a mismatch is a critical
/// rather than undefined behaviour, but it is still a lie to the compiler.
unsafe fn extension<T>(source: *mut ESource, name: &CStr) -> *mut T {
    // SAFETY: the contract above; the extension is owned by the source and
    // lives as long as it does.
    unsafe { e_source_get_extension(source, name.as_ptr()) }.cast()
}

/// A setting no `ESource` property was written from.
///
/// Not a `SourceError`: those are faults in the *user's* account, reported to
/// Evolution as `E_CLIENT_ERROR` codes. This is a fault in this backend — the
/// settings come from `jmap-collection-sync`, not from anything a user typed —
/// and the only thing a caller can do with one is abandon the child and log
/// which setting it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnwritableSetting {
    /// No property of any extension is known for this group and key.
    UnknownProperty {
        group: &'static str,
        key: &'static str,
    },
    /// The property is known, but the value is not one it can hold.
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
