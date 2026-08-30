// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether an account opts into rebasing the session's `apiUrl`/
//! `downloadUrl`/`uploadUrl`/`eventSourceUrl` onto the origin actually
//! connected through: a custom `ESourceExtension`, `[JMAP Rebase]` in the
//! account's own keyfile, with one boolean property, `rebase-urls`.
//!
//! `JMAP_LIVE_SERVER_REBASE_URLS` does the same rewrite (see
//! `jmap_client::ClientBuilder::rebase_urls_to_origin`), but read once from
//! the environment it is one bit for the whole `evolution-source-registry`/
//! factory process — every account at once. Both directions are live in
//! practice: a self-hosted server reached through a tunnel needs the rebase,
//! while a provider that deliberately serves blobs from a different host
//! (Fastmail) must never have it rewritten onto the API origin — so a profile
//! with one of each cannot have both work through the environment variable
//! alone. This extension makes it a property of the account instead,
//! defaulting to off so nothing changes for an existing account;
//! [`crate::source::connect`] ORs it with the environment variable, which
//! stays as a global override for the `--features live-server` harness and
//! the committed probes.
//!
//! Same mechanism `jmap_config::oauth2`'s `[JMAP OAuth2]` extension uses — see
//! that module's docs for why a custom extension rather than a new field on
//! an existing one, and why the property needs `E_SOURCE_PARAM_SETTING` to be
//! read and written by `ESource`'s own (de)serialisation. Registering the
//! type is done lazily, on first read, rather than at module load: checked
//! against the evolution-data-server 3.52.3 source rather than assumed,
//! `e_source_get_extension` creates and populates an extension from the
//! source's own retained `GKeyFile` the first time anything asks for it by
//! name, regardless of when in the process's life the `GType` for that name
//! became registered (`source_parse_dbus_data`'s own comment: "not all the
//! extension classes may be registered at this point... [extensions are]
//! created on-demand in e_source_get_extension()"). Every read here goes
//! through [`rebase_urls`], which registers the type first, so this is
//! self-contained.

use std::ffi::CStr;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use eds_sys::{ESource, ESourceExtension, ESourceExtensionClass, e_source_extension_get_type};
use glib_sys::{GFALSE, GType};
use gobject_sys::{
    G_PARAM_READWRITE, G_PARAM_USER_SHIFT, GObject, GObjectClass, GParamFlags, GParamSpec, GValue,
    g_object_class_install_property, g_param_spec_boolean, g_value_get_boolean,
    g_value_set_boolean,
};

use crate::marshal::extension_if_present;
use crate::subclass::{ObjectSubclass, register_static};
use crate::trampoline::{guard, log_critical};

/// The keyfile group EDS finds this extension by — what an account's
/// `.source` file shows as `[JMAP Rebase]`.
///
/// Not `[Jmap Backend]`, which this was until the collision it caused was
/// found: `e_source_camel_generate_subtype` names the `ESourceCamel` subtype it
/// generates for a protocol `"<Protocol> Backend"`, so the `jmap` provider's own
/// Camel settings already live under exactly that group. EDS files every
/// `ESourceExtension` subclass's `class->name` into one `GHashTable`
/// (`source_find_extension_classes_rec`), where `g_hash_table_insert` keeps the
/// last writer and the order is `g_type_children`'s, so two classes claiming one
/// name is not an error anywhere — it is a name that resolves to whichever type
/// was registered later. A mail store or transport could hand this extension
/// back where its Camel settings were asked for, leaving the account unable to
/// connect with nothing logged; and [`rebase_urls`] could be handed an
/// `ESourceCamel` to read as [`Extension`]. `[JMAP Rebase]` cannot collide with
/// a generated Camel group, and matches the naming `jmap_config::oauth2`'s
/// `[JMAP OAuth2]` already used. `tests/extension_name_collision.rs` in
/// `jmap-backend-collection` pins the rule for both custom extensions.
pub const EXTENSION_NAME: &CStr = c"JMAP Rebase";

/// `E_SOURCE_PARAM_SETTING`, computed rather than bound, as in
/// `jmap_config::oauth2`: `e-source.h` defines it as a plain `#define (1 <<
/// G_PARAM_USER_SHIFT)`, not a symbol, so there is nothing for bindgen to
/// hand back.
const E_SOURCE_PARAM_SETTING: GParamFlags = 1 << (G_PARAM_USER_SHIFT as u32);

const PROPERTY_NAME: &CStr = c"rebase-urls";
const PROP_REBASE_URLS: u32 = 1;

/// The instance struct: nothing but [`ESourceExtension`]'s own state plus the
/// one flag, following the layout contract [`ObjectSubclass`] requires.
#[repr(C)]
pub struct Extension {
    parent: ESourceExtension,
    rebase_urls: AtomicBool,
}

/// The class struct: nothing but [`ESourceExtensionClass`]'s own state.
#[repr(C)]
pub struct ExtensionClass {
    parent_class: ESourceExtensionClass,
}

// SAFETY: both structs are #[repr(C)] and lead with ESourceExtension's own
// instance/class structs, whose layout eds-sys's tests/layout.rs checks
// against `g_type_query`; ESourceExtension derives from GObject. The same
// contract `jmap_config::oauth2::Extension` relies on.
unsafe impl ObjectSubclass for Extension {
    const NAME: &'static CStr = c"JmapRebaseUrlsExtension";
    type Instance = Extension;
    type Class = ExtensionClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_source_extension_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` leads with `ESourceExtensionClass`, which is the
        // same field `source_find_extension_classes_rec` reads to match an
        // extension name to a `GType`.
        unsafe { (*class).parent_class.name = EXTENSION_NAME.as_ptr() };

        // SAFETY: `ExtensionClass` leads with `ESourceExtensionClass`, which
        // leads with `GObjectClass`.
        let object_class = class.cast::<GObjectClass>();
        unsafe {
            (*object_class).set_property = Some(set_property);
            (*object_class).get_property = Some(get_property);
            g_object_class_install_property(
                object_class,
                PROP_REBASE_URLS,
                g_param_spec_boolean(
                    PROPERTY_NAME.as_ptr(),
                    ptr::null(),
                    ptr::null(),
                    GFALSE,
                    G_PARAM_READWRITE | E_SOURCE_PARAM_SETTING,
                ),
            );
        }
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // SAFETY: a freshly zeroed instance — `false`'s own bit pattern, but
        // written explicitly rather than relied on.
        unsafe { ptr::addr_of_mut!((*instance).rebase_urls).write(AtomicBool::new(false)) };
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
    guard("JmapRebaseUrlsExtension::set_property", (), || {
        if id != PROP_REBASE_URLS {
            log_critical(&format!(
                "JmapRebaseUrlsExtension has no property with id {id}; the value is dropped"
            ));
            return;
        }
        // SAFETY: GObject passes an instance of this type; `instance_init`
        // has already run by the time any vfunc can be dispatched on it.
        let extension = unsafe { &*object.cast::<Extension>() };
        // SAFETY: `value` holds the boolean type the pspec above declares.
        let flag = unsafe { g_value_get_boolean(value) } != GFALSE;
        extension.rebase_urls.store(flag, Ordering::Relaxed);
    });
}

/// The reading half, symmetric with [`set_property`].
unsafe extern "C" fn get_property(
    object: *mut GObject,
    id: u32,
    value: *mut GValue,
    _pspec: *mut GParamSpec,
) {
    guard("JmapRebaseUrlsExtension::get_property", (), || {
        if id != PROP_REBASE_URLS {
            log_critical(&format!(
                "JmapRebaseUrlsExtension has no property with id {id}; nothing is read"
            ));
            return;
        }
        // SAFETY: as `set_property`.
        let extension = unsafe { &*object.cast::<Extension>() };
        let flag = extension.rebase_urls.load(Ordering::Relaxed);
        // SAFETY: `value` is the `GValue` GObject is filling for this get.
        unsafe { g_value_set_boolean(value, flag as _) };
    });
}

/// Registers [`Extension`] (or finds it already registered), which is what
/// makes it one `e_source_get_extension`/`e_source_has_extension` can match
/// [`EXTENSION_NAME`] against — see the module docs for why calling this
/// lazily, from [`rebase_urls`], is enough.
pub fn ensure_registered() {
    register_static::<Extension>();
}

/// Whether `source` opts into `apiUrl`/`downloadUrl`/`uploadUrl`/
/// `eventSourceUrl` rebasing (see the module docs). `false` for a source with
/// no `[JMAP Rebase]` group — every account written before this existed, and
/// the default for a new one.
///
/// # Safety
///
/// `source` must be a valid `ESource`, only read from, alive for the call.
pub unsafe fn rebase_urls(source: *mut ESource) -> bool {
    ensure_registered();
    // SAFETY: the contract above; `extension_if_present` only reads.
    match unsafe { extension_if_present::<Extension>(source, EXTENSION_NAME) } {
        None => false,
        // SAFETY: a live extension of this type, by `extension_if_present`'s
        // contract.
        Some(extension) => unsafe { &*extension }.rebase_urls.load(Ordering::Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use eds_sys::{e_source_get_extension, e_source_new_with_uid};
    use gobject_sys::{g_object_set_property, g_object_unref, g_value_init, g_value_unset};

    use super::*;

    struct TestSource(*mut ESource);

    impl TestSource {
        fn new() -> Self {
            let uid = CString::new("jmap-rebase-test-source").expect("no NUL in a literal");
            let mut error = ptr::null_mut();
            // SAFETY: a NUL-terminated uid, no D-Bus object and a GError
            // out-parameter are the documented arguments.
            let source =
                unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
            assert!(!source.is_null(), "e_source_new_with_uid failed");
            Self(source)
        }

        /// Sets the property through the raw GObject API — the door EDS's own
        /// `.source` (de)serialisation uses, not [`rebase_urls`] itself.
        fn set(self, value: bool) -> Self {
            ensure_registered();
            // SAFETY: the type is registered above, and the source is alive.
            let extension = unsafe { e_source_get_extension(self.0, EXTENSION_NAME.as_ptr()) };
            // SAFETY: a live extension of this class, which installs
            // `rebase-urls` as a boolean; the GValue is initialised, filled
            // and unset before it goes out of scope.
            unsafe {
                let mut gvalue: GValue = std::mem::zeroed();
                g_value_init(&mut gvalue, gobject_sys::G_TYPE_BOOLEAN);
                g_value_set_boolean(&mut gvalue, value as _);
                g_object_set_property(extension.cast(), PROPERTY_NAME.as_ptr(), &gvalue);
                g_value_unset(&mut gvalue);
            }
            self
        }
    }

    impl Drop for TestSource {
        fn drop(&mut self) {
            // SAFETY: we hold the only reference.
            unsafe { g_object_unref(self.0.cast()) };
        }
    }

    #[test]
    fn a_source_with_no_jmap_backend_group_does_not_rebase() {
        let source = TestSource::new();
        assert!(!unsafe { rebase_urls(source.0) });
    }

    #[test]
    fn a_source_with_rebase_urls_true_rebases() {
        let source = TestSource::new().set(true);
        assert!(unsafe { rebase_urls(source.0) });
    }

    #[test]
    fn two_sources_in_the_same_process_rebase_independently() {
        // The scenario the docs describe: a tunnelled self-hosted account
        // that needs the rebase, alongside one that must not have it,
        // reached through the same process — nothing here shares state
        // across sources.
        let tunnelled = TestSource::new().set(true);
        let ordinary = TestSource::new();

        assert!(unsafe { rebase_urls(tunnelled.0) });
        assert!(!unsafe { rebase_urls(ordinary.0) });
    }
}
