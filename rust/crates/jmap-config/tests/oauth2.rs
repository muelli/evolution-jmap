// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The `[JMAP OAuth2]` extension, against real `ESource`s — both through this
// crate's own `apply`/`read` door and through the raw GObject property API,
// which is the door EDS's own `.source` (de)serialisation uses. The second
// is what actually proves the properties are wired: a typo'd name or a
// missing flag would still let `apply`/`read` agree with themselves while
// persisting nothing, which is exactly the "nearly silent" failure
// `jmap_mail::settings` warns about for the same shape of code.

use std::ffi::{CStr, CString};
use std::ptr;

use eds_sys::{ESource, e_source_get_extension, e_source_has_extension, e_source_new_with_uid};
use glib_sys::GFALSE;
use gobject_sys::{
    G_TYPE_STRING, GObject, GValue, g_object_get_property, g_object_set_property, g_object_unref,
    g_value_get_string, g_value_init, g_value_set_string, g_value_unset,
};
use jmap_config::oauth2::{Config, EXTENSION_NAME, apply, read};

struct TestSource(*mut ESource);

impl TestSource {
    fn new() -> Self {
        // In the real module this is done once, at load time, alongside the
        // rest of this project's types — see `oauth2::ensure_registered`'s own
        // doc comment for why an unregistered type is invisible to EDS's own
        // keyfile parser, not only to this crate's `apply`/`read`. Called here
        // because this test also drives the extension through the raw
        // GObject property API, which — unlike `apply`/`read` — does not
        // register the type itself.
        jmap_config::oauth2::ensure_registered();

        let uid = CString::new("jmap-account").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, no D-Bus object and a GError
        // out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    fn written(self, config: &Config) -> Self {
        // SAFETY: a live source.
        unsafe { apply(self.0, config) };
        self
    }

    fn config(&self) -> Config {
        // SAFETY: a live source.
        unsafe { read(self.0) }
    }

    /// Reads a property by name through the raw GObject API — the same path
    /// EDS's own keyfile writer uses, and not the one [`Self::config`] takes.
    fn property(&self, name: &str) -> Option<String> {
        let name = CString::new(name).expect("no NUL in a literal");
        // SAFETY: a live extension instance and a freshly initialised GValue
        // of the type every property here declares; `g_object_get_property`
        // fills it and the value is read out before it is unset.
        unsafe {
            let extension = self.extension();
            let mut value: GValue = std::mem::zeroed();
            g_value_init(&mut value, G_TYPE_STRING);
            g_object_get_property(extension.cast(), name.as_ptr(), &mut value);
            let text = g_value_get_string(&value);
            let result =
                (!text.is_null()).then(|| CStr::from_ptr(text).to_string_lossy().into_owned());
            g_value_unset(&mut value);
            result
        }
    }

    /// Writes a property by name through the raw GObject API.
    fn set_property(&self, name: &str, value: &str) {
        let name = CString::new(name).expect("no NUL in a literal");
        let value_c = CString::new(value).expect("no NUL in a literal");
        // SAFETY: as `property`, and the string outlives the call that copies
        // it in.
        unsafe {
            let extension = self.extension();
            let mut gvalue: GValue = std::mem::zeroed();
            g_value_init(&mut gvalue, G_TYPE_STRING);
            g_value_set_string(&mut gvalue, value_c.as_ptr());
            g_object_set_property(extension.cast(), name.as_ptr(), &gvalue);
            g_value_unset(&mut gvalue);
        }
    }

    /// The extension instance itself, for the two raw-property helpers above.
    fn extension(&self) -> *mut GObject {
        // SAFETY: a live source; `EXTENSION_NAME` is the group this crate's
        // extension registers under.
        unsafe { e_source_get_extension(self.0, EXTENSION_NAME.as_ptr()).cast() }
    }

    fn has_extension(&self) -> bool {
        // SAFETY: a live source.
        unsafe { e_source_has_extension(self.0, EXTENSION_NAME.as_ptr()) != GFALSE }
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: this holds the only reference.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

fn config() -> Config {
    Config {
        client_id: Some("client-abc123".to_owned()),
        client_secret: Some("s3cret".to_owned()),
        authorization_endpoint: Some("https://jmap.example.com/authorize".to_owned()),
        token_endpoint: Some("https://jmap.example.com/token".to_owned()),
        redirect_uri: Some("https://client.example.org/callback".to_owned()),
    }
}

#[test]
fn a_source_that_says_nothing_reads_as_a_config_with_nothing_in_it() {
    let source = TestSource::new();

    assert_eq!(source.config(), Config::default());
}

#[test]
fn reading_a_config_does_not_add_the_extension() {
    let source = TestSource::new();

    let _ = source.config();

    assert!(
        !source.has_extension(),
        "reading created the [JMAP OAuth2] group"
    );
}

#[test]
fn a_config_that_was_written_is_the_config_that_is_read_back() {
    let source = TestSource::new().written(&config());

    assert_eq!(source.config(), config());
}

#[test]
fn committing_a_config_that_dropped_its_secret_clears_the_one_that_was_there() {
    let source = TestSource::new().written(&config());

    let second = Config {
        client_secret: None,
        ..config()
    };
    let source = source.written(&second);

    assert_eq!(source.config(), second);
}

#[test]
fn every_field_is_reachable_through_the_gobject_property_it_was_installed_as() {
    // The property names `class_init` installed, matched against the fields
    // `apply` writes — proof the two agree, rather than each independently
    // claiming to.
    let source = TestSource::new().written(&config());

    assert_eq!(
        source.property("client-id").as_deref(),
        Some("client-abc123")
    );
    assert_eq!(source.property("client-secret").as_deref(), Some("s3cret"));
    assert_eq!(
        source.property("authorization-endpoint").as_deref(),
        Some("https://jmap.example.com/authorize")
    );
    assert_eq!(
        source.property("token-endpoint").as_deref(),
        Some("https://jmap.example.com/token")
    );
    assert_eq!(
        source.property("redirect-uri").as_deref(),
        Some("https://client.example.org/callback")
    );
}

#[test]
fn a_value_set_through_the_gobject_property_is_read_back_through_this_crates_own_door() {
    // The direction EDS's own `.source` parser writes in: restoring an
    // account from disk sets properties, it does not call `apply`.
    let source = TestSource::new();
    source.set_property("client-id", "restored-client-id");

    assert_eq!(
        source.config().client_id.as_deref(),
        Some("restored-client-id")
    );
}
