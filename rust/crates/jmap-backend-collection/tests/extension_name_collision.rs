// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Two `ESourceExtension` subclasses must not have the same name.
//
// To find the class for an extension name, EDS walks all subclasses of
// `ESourceExtension`. It puts the `class->name` of each subclass into one
// `GHashTable` (`source_find_extension_classes_rec` in
// `libedataserver/e-source.c`). EDS builds this table again on each
// `e_source_get_extension` call. `g_hash_table_insert` keeps the last value.
// Thus, if two classes have one name, no error occurs. The name goes to the
// class that `g_type_children` finds last. That depends on the registration
// state at the first lookup, which a reader cannot see.
//
// This crate has two custom extensions. `e_source_camel_generate_subtype` gives
// the name `"<Protocol> Backend"` to the `ESourceCamel` subtype of a protocol.
// Thus the Camel settings of the `jmap` provider are in `[Jmap Backend]`. That
// name must not be the name of the `jmap_backend_core::rebase` extension (it
// was once). If it is, two failures occur. A mail store or transport gets no
// Camel settings and cannot connect, and no log line shows the cause. Also,
// `rebase_urls` casts an `ESourceCamel` to its own instance struct.
//
// This file is a separate test binary intentionally. The collision occurs only
// when both types are registered. In a shared process, the result depends on
// which test ran first, so the failure becomes intermittent.

use std::ffi::CStr;
use std::ptr;
use std::sync::OnceLock;

use eds_sys::{
    ESource, ESourceCamel, e_source_camel_generate_subtype, e_source_camel_get_extension_name,
    e_source_camel_get_settings, e_source_get_extension, e_source_has_extension,
    e_source_new_with_uid, g_object_unref,
};
use glib_sys::GFALSE;
use jmap_backend_collection::prepare_mail::MAIL_BACKEND_NAME;
use jmap_mail::settings::settings_type;

mod common;
use common::with_timeout;

/// A bare `ESource` with only its own group. These tests only request an
/// extension by name, and this source is sufficient for that.
struct Source(*mut ESource);

impl Source {
    fn new(uid: &CStr) -> Self {
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, a NULL main context (the default
        // context) and a GError out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: this struct holds the only reference.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// The extension name that EDS generates for the protocol of this crate. Both
/// custom extensions are registered in the order that shows a collision.
///
/// If two classes have the same name, `g_type_children` selects the class by
/// registration order. The generated `ESourceCamel` subtype becomes a child of
/// `ESourceExtension` only at the first request for it. If it is registered
/// before `JmapRebaseUrlsExtension`, the rebase extension is the later insert and
/// gets the name. If it is registered after, the subtype gets the name. All
/// tests below call this function, so the order is the same for the full binary.
///
/// The `OnceLock` also prevents a race, as in `tests/recipe.rs` and
/// `tests/mail_child.rs`. `e_source_camel_generate_subtype` finds and
/// registers the type name in two steps. If two threads do not find the type,
/// both call `g_type_register_static`, and one of them gets `G_TYPE_INVALID`.
fn camel_extension_name() -> &'static CStr {
    static NAME: OnceLock<usize> = OnceLock::new();
    let name = *NAME.get_or_init(|| {
        // SAFETY: a NUL-terminated protocol name and a GType derived from
        // CamelSettings, which `settings_type` registers. The returned name is
        // interned and never freed.
        let name = unsafe {
            let gtype =
                e_source_camel_generate_subtype(MAIL_BACKEND_NAME.as_ptr(), settings_type());
            assert_ne!(
                gtype, 0,
                "no ESourceCamel subtype was generated for the jmap protocol"
            );
            e_source_camel_get_extension_name(MAIL_BACKEND_NAME.as_ptr()) as usize
        };
        jmap_backend_core::rebase::ensure_registered();
        jmap_config::oauth2::ensure_registered();
        name
    });
    // SAFETY: an interned, NUL-terminated string that is never freed.
    unsafe { CStr::from_ptr(name as *const _) }
}

#[test]
fn the_rebase_extension_does_not_answer_to_the_camel_subtypes_name() {
    with_timeout(|| {
        // Compare the names as strings, before instantiation. A failure of this
        // check gives the cause directly.
        assert_ne!(
            jmap_backend_core::rebase::EXTENSION_NAME,
            camel_extension_name(),
            "the rebase extension and the generated ESourceCamel subtype claim \
             one keyfile group, so EDS resolves the name to whichever GType was \
             registered last"
        );
    });
}

#[test]
fn the_oauth2_extension_does_not_answer_to_the_camel_subtypes_name() {
    with_timeout(|| {
        // Apply the same rule to the other custom extension. A third extension
        // will have this form. Only its spelling makes `[JMAP OAuth2]` safe.
        assert_ne!(
            jmap_config::oauth2::EXTENSION_NAME,
            camel_extension_name(),
            "the OAuth2 extension and the generated ESourceCamel subtype claim \
             one keyfile group"
        );
    });
}

#[test]
fn a_registered_rebase_extension_leaves_the_camel_settings_reachable() {
    with_timeout(|| {
        // The failure as EDS causes it. `camel_extension_name` registers the
        // generated subtype first and the rebase extension second. Thus the
        // rebase extension is the later insert into the name table and gets the
        // name if it has the same name.
        let name = camel_extension_name();
        let source = Source::new(c"jmap-extension-collision-camel");

        // SAFETY: a live source and the interned extension name of a registered
        // `ESourceCamel` subtype. The source creates the extension on demand and
        // owns it and its settings object.
        let settings = unsafe {
            let extension: *mut ESourceCamel =
                e_source_get_extension(source.0, name.as_ptr()).cast();
            assert!(!extension.is_null(), "the jmap subtype is not registered");
            e_source_camel_get_settings(extension)
        };
        assert!(
            !settings.is_null(),
            "asking for {name:?} did not return the ESourceCamel subtype, so a \
             mail source's Camel settings are unreachable"
        );
    });
}

#[test]
fn a_camel_configured_source_does_not_look_like_a_rebase_source() {
    with_timeout(|| {
        // The other direction, where the failure is undefined behaviour.
        // `rebase_urls` casts the return value of `e_source_get_extension` to its
        // own instance struct. Thus a source with only the Camel group must not
        // report the rebase extension as present.
        let name = camel_extension_name();
        let source = Source::new(c"jmap-extension-collision-rebase");
        // SAFETY: a live source and an interned extension name. The creation of
        // the Camel extension writes its group to the source.
        unsafe { e_source_get_extension(source.0, name.as_ptr()) };

        // SAFETY: a live source and a `&CStr`.
        let present = unsafe {
            e_source_has_extension(source.0, jmap_backend_core::rebase::EXTENSION_NAME.as_ptr())
        };
        assert_eq!(
            present, GFALSE,
            "a source with only the ESourceCamel group reports the rebase \
             extension as present, so `rebase_urls` would read an ESourceCamel \
             as its own instance struct"
        );
        assert!(
            // SAFETY: a live source, only read from.
            !unsafe { jmap_backend_core::rebase::rebase_urls(source.0) },
            "a source that never opted into rebasing reports that it did"
        );
    });
}
