// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Two `ESourceExtension` subclasses may not answer to the same name.
//
// EDS matches an extension name to a class by walking every subclass of
// `ESourceExtension` and filing each one's `class->name` into a single
// `GHashTable` — `source_find_extension_classes_rec` in
// `libedataserver/e-source.c`, rebuilt on every `e_source_get_extension` call.
// `g_hash_table_insert` keeps the last writer, so two classes claiming one name
// is not an error anywhere: it is whichever of them `g_type_children` happens to
// reach last, which depends on what has been registered by the time the first
// lookup runs and therefore on nothing a reader can see.
//
// This crate has two custom extensions, and one of them used to collide with a
// name it did not choose. `e_source_camel_generate_subtype` names the
// `ESourceCamel` subtype it generates for a protocol `"<Protocol> Backend"`, so
// the `jmap` provider's Camel settings live under `[Jmap Backend]` — which is
// what `jmap_backend_core::rebase`'s extension had also called itself. The
// visible symptom was a mail store or transport whose Camel settings came back
// as no settings at all, an account that cannot connect with nothing logged; the
// invisible one was `rebase_urls` casting an `ESourceCamel` to its own instance
// struct.
//
// So this file is one binary on purpose. The collision only bites once both
// types are registered, and a test that shares a process with others is a test
// whose answer depends on which of them ran first — which is exactly why this
// showed up as an eight-in-twenty flake in `tests/recipe.rs` rather than as the
// deterministic failure it is.

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

/// A bare `ESource` with no groups but its own — enough to ask for an extension
/// by name, which is all these tests do.
struct Source(*mut ESource);

impl Source {
    fn new(uid: &CStr) -> Self {
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid; no D-Bus object, which is what EDS's own
        // callers pass for a source that is not on a bus; a GError out-parameter.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: we hold the only reference.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// The extension name EDS generates for this crate's protocol, with both custom
/// extensions registered in the one order that exposes a collision.
///
/// Which of two same-named classes EDS resolves the name to is decided by
/// `g_type_children`, and that is registration order: `ESourceCamel` is
/// abstract and lazy, so the type only joins `ESourceExtension`'s children when
/// something first asks for it. Register it before `JmapRebaseUrlsExtension` and
/// the rebase extension is the later insert and wins the name; register it after
/// and the generated subtype does. So a test that leaves the order to whichever
/// of its siblings happened to run first is a test that reports a collision
/// half the time. Every test below goes through here, and this fixes the
/// order once for the binary.
///
/// The `OnceLock` is also what `tests/recipe.rs` and `tests/mail_child.rs` need
/// it for: `e_source_camel_generate_subtype` looks the type name up and
/// registers it in two steps, so two threads that both find it missing both call
/// `g_type_register_static` and the loser is handed `G_TYPE_INVALID`.
fn camel_extension_name() -> &'static CStr {
    static NAME: OnceLock<usize> = OnceLock::new();
    let name = *NAME.get_or_init(|| {
        // SAFETY: a NUL-terminated protocol name and a GType derived from
        // CamelSettings, which is what `settings_type` registers; the name
        // handed back is interned and never freed.
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
        // The names as strings, before anything is instantiated: the cheap half
        // of the check, and the one whose failure names the cause outright.
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
        // The other custom extension, checked by the same rule rather than by
        // inspection — it is the shape a third one would be written to, and
        // `[JMAP OAuth2]` is only safe by luck of spelling.
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
        // The failure as EDS produces it, with the two types registered in the
        // order `camel_extension_name` fixes — the generated subtype first, the
        // rebase extension second, so the rebase extension is the later insert
        // into the name table and takes the name if it claims one.
        let name = camel_extension_name();
        let source = Source::new(c"jmap-extension-collision-camel");

        // SAFETY: a live source and the interned extension name of a registered
        // `ESourceCamel` subtype; the extension is created on demand and owned by
        // the source, and so is the settings object it holds.
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
        // The other direction, and the one that is undefined behaviour rather
        // than a missing value: `rebase_urls` casts whatever
        // `e_source_get_extension` hands back to its own instance struct, so a
        // source carrying only the Camel group must not report the rebase
        // extension as present.
        let name = camel_extension_name();
        let source = Source::new(c"jmap-extension-collision-rebase");
        // SAFETY: a live source and an interned extension name; creating the
        // Camel extension is what writes its group onto the source.
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
