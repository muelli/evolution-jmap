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

use eds_sys::ESource;

use crate::setting_extension::{self, Setting, SettingExtension};

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

const PROPERTY_NAME: &CStr = c"rebase-urls";

/// The rebase opt-in as a [`Setting`]: one boolean property, `rebase-urls`.
pub struct RebaseUrls;

impl Setting for RebaseUrls {
    const TYPE_NAME: &'static CStr = c"JmapRebaseUrlsExtension";
    const EXTENSION_NAME: &'static CStr = EXTENSION_NAME;
    const PROPERTY: &'static CStr = PROPERTY_NAME;
    type Value = bool;
}

/// The registered extension type of [`RebaseUrls`].
pub type Extension = SettingExtension<RebaseUrls>;

/// Registers [`Extension`] (or finds it already registered). After this,
/// `e_source_get_extension`/`e_source_has_extension` can match
/// [`EXTENSION_NAME`] to it. The module docs tell why a lazy call from
/// [`rebase_urls`] is sufficient.
pub fn ensure_registered() {
    setting_extension::ensure_registered::<RebaseUrls>();
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
    // SAFETY: the contract above; a read does not change the source.
    unsafe { setting_extension::read::<RebaseUrls>(source) }.unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use eds_sys::{e_source_get_extension, e_source_parameter_to_key, e_source_to_string};
    use glib_sys::g_free;
    use gobject_sys::{
        GValue, g_object_set_property, g_value_init, g_value_set_boolean, g_value_unset,
    };

    use super::*;
    use crate::source::OwnedSource;

    struct TestSource(OwnedSource);

    impl TestSource {
        fn new() -> Self {
            Self(OwnedSource::new("jmap-rebase-test-source"))
        }

        /// Sets the property through the raw GObject API. The `.source`
        /// (de)serialisation of EDS uses this API, not [`rebase_urls`].
        fn set(self, value: bool) -> Self {
            ensure_registered();
            // SAFETY: the type is registered above, and the source is alive.
            let extension =
                unsafe { e_source_get_extension(self.0.as_ptr(), EXTENSION_NAME.as_ptr()) };
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
                let text = e_source_to_string(self.0.as_ptr(), ptr::null_mut());
                assert!(!text.is_null(), "e_source_to_string returned NULL");
                let owned = CStr::from_ptr(text).to_string_lossy().into_owned();
                g_free(text.cast());
                owned
            }
        }
    }

    #[test]
    fn a_source_with_no_jmap_backend_group_does_not_rebase() {
        let source = TestSource::new();
        assert!(!unsafe { rebase_urls(source.0.as_ptr()) });
    }

    #[test]
    fn a_source_with_rebase_urls_true_rebases() {
        let source = TestSource::new().set(true);
        assert!(unsafe { rebase_urls(source.0.as_ptr()) });
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

        assert!(unsafe { rebase_urls(tunnelled.0.as_ptr()) });
        assert!(!unsafe { rebase_urls(ordinary.0.as_ptr()) });
    }
}
