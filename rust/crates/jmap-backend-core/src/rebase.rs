// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether an account opts in to the rebase of the
//! `apiUrl`/`downloadUrl`/`uploadUrl`/`eventSourceUrl` of the session onto
//! the origin of the actual connection. The opt-in is a custom
//! `ESourceExtension`, `[JMAP Rebase]` in the keyfile of the account, with
//! one boolean property, `rebase-urls`.
//!
//! `JMAP_LIVE_SERVER_REBASE_URLS` does the same rewrite (see
//! `jmap_client::ClientBuilder::rebase_urls_to_origin`). But the code reads
//! it one time from the environment, so it is one bit for the full
//! `evolution-source-registry`/factory process: all accounts together. In
//! practice, both directions are necessary. A self-hosted server that a
//! tunnel connects to needs the rebase. A provider that intentionally serves
//! blobs from a different host (Fastmail) must never have them rewritten
//! onto the API origin. Thus with only the environment variable, a profile
//! with one account of each type cannot make both work. This extension makes
//! the rebase a property of the account. Its default is off, so nothing
//! changes for an existing account. [`crate::source::connect`] ORs it with
//! the environment variable. The variable stays as a global override for the
//! `--features live-server` harness and the committed probes.
//!
//! The `[JMAP OAuth2]` extension of `jmap_config::oauth2` uses the same
//! mechanism. The docs of that module tell why it is a custom extension and
//! not a new field on an existing one. They also tell why the property needs
//! `E_SOURCE_PARAM_SETTING`, so that the (de)serialisation of `ESource` reads
//! and writes it. The type registration occurs lazily, on the first read, not
//! at module load. This is correct (checked against the evolution-data-server
//! 3.52.3 source, not assumed): `e_source_get_extension` creates an extension
//! from the retained `GKeyFile` of the source and fills it the first time a
//! caller asks for that name. The time when the `GType` for that name became
//! registered in the life of the process is not important. (The comment in
//! `source_parse_dbus_data` says: "not all the extension classes may be
//! registered at this point... [extensions are] created on-demand in
//! e_source_get_extension()".) Every read here goes through [`rebase_urls`],
//! which registers the type first. Thus this module is self-contained.

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

/// The keyfile group that EDS uses to find this extension. The `.source`
/// file of an account shows it as `[JMAP Rebase]`.
///
/// The name was `[Jmap Backend]` until a collision was found.
/// `e_source_camel_generate_subtype` gives the `ESourceCamel` subtype that it
/// generates for a protocol the name `"<Protocol> Backend"`. Thus the Camel
/// settings of the `jmap` provider are already in exactly that group. EDS
/// puts the `class->name` of every `ESourceExtension` subclass into one
/// `GHashTable` (`source_find_extension_classes_rec`). There,
/// `g_hash_table_insert` keeps the last writer, in the sequence of
/// `g_type_children`. Thus two classes with one name cause no error: the name
/// resolves to the type that was registered last. A mail store or transport
/// could then return this extension when it asks for its Camel settings. The
/// account then cannot connect, and nothing is logged. Also, [`rebase_urls`]
/// could get an `ESourceCamel` to read as [`Extension`]. `[JMAP Rebase]`
/// cannot collide with a generated Camel group. It also matches the name
/// `[JMAP OAuth2]` of `jmap_config::oauth2`.
/// `tests/extension_name_collision.rs` in `jmap-backend-collection` locks
/// the rule for both custom extensions.
pub const EXTENSION_NAME: &CStr = c"JMAP Rebase";

/// `E_SOURCE_PARAM_SETTING`, computed and not bound, as in
/// `jmap_config::oauth2`. `e-source.h` defines it as a plain `#define (1 <<
/// G_PARAM_USER_SHIFT)`, not a symbol, so bindgen cannot supply it.
const E_SOURCE_PARAM_SETTING: GParamFlags = 1 << (G_PARAM_USER_SHIFT as u32);

const PROPERTY_NAME: &CStr = c"rebase-urls";
const PROP_REBASE_URLS: u32 = 1;

/// The instance struct: only the state of [`ESourceExtension`] and the one
/// flag, in the layout that [`ObjectSubclass`] requires.
#[repr(C)]
pub struct Extension {
    parent: ESourceExtension,
    rebase_urls: AtomicBool,
}

/// The class struct: only the state of [`ESourceExtensionClass`].
#[repr(C)]
pub struct ExtensionClass {
    parent_class: ESourceExtensionClass,
}

// SAFETY: both structs are #[repr(C)] and start with the instance/class
// structs of ESourceExtension. tests/layout.rs of eds-sys checks their layout
// against `g_type_query`. ESourceExtension derives from GObject.
// `jmap_config::oauth2::Extension` uses the same contract.
unsafe impl ObjectSubclass for Extension {
    const NAME: &'static CStr = c"JmapRebaseUrlsExtension";
    type Instance = Extension;
    type Class = ExtensionClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_source_extension_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` starts with `ESourceExtensionClass`. This is the
        // field that `source_find_extension_classes_rec` reads to match an
        // extension name to a `GType`.
        unsafe { (*class).parent_class.name = EXTENSION_NAME.as_ptr() };

        // SAFETY: `ExtensionClass` starts with `ESourceExtensionClass`, which
        // starts with `GObjectClass`.
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
        // SAFETY: a new zeroed instance. Zero is the bit pattern of `false`,
        // but the code writes it explicitly and does not assume it.
        unsafe { ptr::addr_of_mut!((*instance).rebase_urls).write(AtomicBool::new(false)) };
    }
}

/// Sends a property write to the storage of the instance.
///
/// This has a guard, as every function that C calls in this crate has. It
/// runs inside `g_object_set`, which the `.source` parser of EDS calls when
/// it restores an account. A panic that unwinds out of it is undefined
/// behaviour in a process that holds the mail of other users.
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
        // SAFETY: GObject passes an instance of this type. `instance_init`
        // runs before a vfunc can be dispatched on it.
        let extension = unsafe { &*object.cast::<Extension>() };
        // SAFETY: `value` holds the boolean type that the pspec declares.
        let flag = unsafe { g_value_get_boolean(value) } != GFALSE;
        extension.rebase_urls.store(flag, Ordering::Relaxed);
    });
}

/// The read half, symmetric with [`set_property`].
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
        // SAFETY: `value` is the `GValue` that GObject fills for this get.
        unsafe { g_value_set_boolean(value, flag as _) };
    });
}

/// Registers [`Extension`] (or finds it already registered). After this,
/// `e_source_get_extension`/`e_source_has_extension` can match
/// [`EXTENSION_NAME`] to it. The module docs tell why a lazy call from
/// [`rebase_urls`] is sufficient.
pub fn ensure_registered() {
    register_static::<Extension>();
}

/// Whether `source` opts in to the rebase of
/// `apiUrl`/`downloadUrl`/`uploadUrl`/`eventSourceUrl` (see the module docs).
/// `false` for a source with no `[JMAP Rebase]` group. This includes every
/// account written before this extension existed, and it is the default for
/// a new account.
///
/// # Safety
///
/// `source` must be a valid `ESource` that stays alive for the call. The
/// call only reads it.
pub unsafe fn rebase_urls(source: *mut ESource) -> bool {
    ensure_registered();
    // SAFETY: the contract above; `extension_if_present` only reads.
    match unsafe { extension_if_present::<Extension>(source, EXTENSION_NAME) } {
        None => false,
        // SAFETY: a live extension of this type, by the contract of
        // `extension_if_present`.
        Some(extension) => unsafe { &*extension }.rebase_urls.load(Ordering::Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use eds_sys::{
        e_source_get_extension, e_source_new_with_uid, e_source_parameter_to_key,
        e_source_to_string,
    };
    use glib_sys::g_free;
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

        /// Sets the property through the raw GObject API. The `.source`
        /// (de)serialisation of EDS uses this API, not [`rebase_urls`].
        fn set(self, value: bool) -> Self {
            ensure_registered();
            // SAFETY: the type is registered above, and the source is alive.
            let extension = unsafe { e_source_get_extension(self.0, EXTENSION_NAME.as_ptr()) };
            // SAFETY: a live extension of this class, which installs
            // `rebase-urls` as a boolean. The code initialises, fills and
            // unsets the GValue before it goes out of scope.
            unsafe {
                let mut gvalue: GValue = std::mem::zeroed();
                g_value_init(&mut gvalue, gobject_sys::G_TYPE_BOOLEAN);
                g_value_set_boolean(&mut gvalue, value as _);
                g_object_set_property(extension.cast(), PROPERTY_NAME.as_ptr(), &gvalue);
                g_value_unset(&mut gvalue);
            }
            self
        }

        /// The source as EDS would write it to disk: the `.source` keyfile
        /// that Evolution reads again after a restart.
        fn serialised(&self) -> String {
            // SAFETY: a live source. `e_source_to_string` is `(transfer
            // full)`. The result is NUL-terminated, and the code reads and
            // frees it here.
            unsafe {
                let text = e_source_to_string(self.0, ptr::null_mut());
                assert!(!text.is_null(), "e_source_to_string returned NULL");
                let owned = CStr::from_ptr(text).to_string_lossy().into_owned();
                g_free(text.cast());
                owned
            }
        }
    }

    impl Drop for TestSource {
        fn drop(&mut self) {
            // SAFETY: this value holds the only reference.
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
    fn rebase_urls_true_persists_into_the_keyfile_an_evolution_restart_reads_back() {
        // `rebase_urls` is useful across restarts only if its flag gets to
        // the `.source` file of the account. A property without
        // `E_SOURCE_PARAM_SETTING` would pass every other test here, but it
        // would change back to `false` on every boot without a warning.
        let source = TestSource::new().set(true);

        let text = source.serialised();

        let group = format!("[{}]", EXTENSION_NAME.to_string_lossy());
        assert!(
            text.contains(&group),
            "the extension wrote no {group} group into the keyfile:\n{text}"
        );
        // SAFETY: a NUL-terminated property name. The result is
        // `(transfer full)`.
        let key = unsafe {
            let raw = e_source_parameter_to_key(PROPERTY_NAME.as_ptr());
            assert!(!raw.is_null(), "e_source_parameter_to_key returned NULL");
            let key = CStr::from_ptr(raw).to_string_lossy().into_owned();
            g_free(raw.cast());
            key
        };
        assert!(
            text.contains(&format!("{key}=true")),
            "{key} did not persist into the keyfile:\n{text}"
        );
    }

    #[test]
    fn two_sources_in_the_same_process_rebase_independently() {
        // The scenario in the docs: in the same process, a tunnelled
        // self-hosted account that needs the rebase, and an account that
        // must not have it. No state is shared between sources.
        let tunnelled = TestSource::new().set(true);
        let ordinary = TestSource::new();

        assert!(unsafe { rebase_urls(tunnelled.0) });
        assert!(!unsafe { rebase_urls(ordinary.0) });
    }
}
