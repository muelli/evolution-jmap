// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Two account settings use `SettingExtension`: the rebase opt-in and the due
// time of a deferred delete. Each test here defines its own setting.

use std::ffi::CStr;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use eds_sys::{e_source_get_extension, e_source_parameter_to_key, e_source_to_string};
use glib_sys::{g_free, gpointer};
use gobject_sys::{GObject, GParamSpec, g_signal_connect_data};
use jmap_backend_core::setting_extension::{self, Setting, SettingExtension};
use jmap_backend_core::source::OwnedSource;

struct Flag;

impl Setting for Flag {
    const TYPE_NAME: &'static CStr = c"JmapTestFlagSettingExtension";
    const EXTENSION_NAME: &'static CStr = c"JMAP Test Flag";
    const PROPERTY: &'static CStr = c"flag";
    type Value = bool;
}

struct Stamp;

impl Setting for Stamp {
    const TYPE_NAME: &'static CStr = c"JmapTestStampSettingExtension";
    const EXTENSION_NAME: &'static CStr = c"JMAP Test Stamp";
    const PROPERTY: &'static CStr = c"stamp";
    type Value = i64;
}

jmap_backend_core::initialise_parents_before_main!(SettingExtension<Flag>, SettingExtension<Stamp>);

/// The source as EDS writes it to its `.source` file.
fn serialised(source: &OwnedSource) -> String {
    // SAFETY: a live source. The result is `(transfer full)` and NUL-terminated.
    unsafe {
        let text = e_source_to_string(source.as_ptr(), ptr::null_mut());
        assert!(!text.is_null(), "e_source_to_string returned NULL");
        let owned = CStr::from_ptr(text).to_string_lossy().into_owned();
        g_free(text.cast());
        owned
    }
}

/// The keyfile key that EDS writes for `property`.
fn key_of(property: &CStr) -> String {
    // SAFETY: a NUL-terminated name. The result is `(transfer full)`.
    unsafe {
        let raw = e_source_parameter_to_key(property.as_ptr());
        assert!(!raw.is_null(), "e_source_parameter_to_key returned NULL");
        let key = CStr::from_ptr(raw).to_string_lossy().into_owned();
        g_free(raw.cast());
        key
    }
}

#[test]
fn a_source_without_the_group_reads_none_and_a_read_adds_no_group() {
    let source = OwnedSource::new("jmap-setting-absent");

    // SAFETY: a live source.
    assert_eq!(
        unsafe { setting_extension::read::<Flag>(source.as_ptr()) },
        None
    );
    assert_eq!(
        unsafe { setting_extension::read::<Stamp>(source.as_ptr()) },
        None
    );

    let text = serialised(&source);
    assert!(
        !text.contains("[JMAP Test"),
        "a read added a group:\n{text}"
    );
}

#[test]
fn a_written_value_reads_back() {
    let source = OwnedSource::new("jmap-setting-round-trip");

    // SAFETY: a live source.
    unsafe {
        setting_extension::write::<Flag>(source.as_ptr(), true);
        setting_extension::write::<Stamp>(source.as_ptr(), -1_700_000_000);
    }

    // SAFETY: a live source.
    assert_eq!(
        unsafe { setting_extension::read::<Flag>(source.as_ptr()) },
        Some(true)
    );
    assert_eq!(
        unsafe { setting_extension::read::<Stamp>(source.as_ptr()) },
        Some(-1_700_000_000)
    );
}

#[test]
fn a_new_extension_holds_the_default() {
    let source = OwnedSource::new("jmap-setting-default");
    setting_extension::ensure_registered::<Flag>();
    setting_extension::ensure_registered::<Stamp>();

    // SAFETY: the types are registered, and the source is alive. The source owns
    // the extensions that this adds.
    unsafe {
        e_source_get_extension(source.as_ptr(), Flag::EXTENSION_NAME.as_ptr());
        e_source_get_extension(source.as_ptr(), Stamp::EXTENSION_NAME.as_ptr());
    }

    // SAFETY: a live source.
    assert_eq!(
        unsafe { setting_extension::read::<Flag>(source.as_ptr()) },
        Some(false)
    );
    assert_eq!(
        unsafe { setting_extension::read::<Stamp>(source.as_ptr()) },
        Some(0)
    );
}

#[test]
fn a_written_value_persists_into_the_keyfile() {
    // Only a property with `E_SOURCE_PARAM_SETTING` gets to the `.source` file.
    let source = OwnedSource::new("jmap-setting-persists");

    // SAFETY: a live source.
    unsafe {
        setting_extension::write::<Flag>(source.as_ptr(), true);
        setting_extension::write::<Stamp>(source.as_ptr(), 1_700_000_000);
    }

    let text = serialised(&source);
    for expected in [
        "[JMAP Test Flag]".to_owned(),
        format!("{}=true", key_of(Flag::PROPERTY)),
        "[JMAP Test Stamp]".to_owned(),
        format!("{}=1700000000", key_of(Stamp::PROPERTY)),
    ] {
        assert!(
            text.contains(&expected),
            "no {expected} in the keyfile:\n{text}"
        );
    }
}

static NOTIFIED: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn count_notify(_object: *mut GObject, _pspec: *mut GParamSpec, _data: gpointer) {
    NOTIFIED.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn a_write_emits_notify_for_the_property() {
    // The registry sends a changed source to Evolution on `notify`.
    let source = OwnedSource::new("jmap-setting-notify");
    setting_extension::ensure_registered::<Stamp>();
    // SAFETY: the type is registered, and the source is alive and owns the result.
    let extension =
        unsafe { e_source_get_extension(source.as_ptr(), Stamp::EXTENSION_NAME.as_ptr()) };
    // SAFETY: a live object, and a handler with the `notify` signature. The
    // handler uses no data. The source and its extension die at the end of the test.
    unsafe {
        let handler: unsafe extern "C" fn(*mut GObject, *mut GParamSpec, gpointer) = count_notify;
        g_signal_connect_data(
            extension.cast(),
            c"notify::stamp".as_ptr(),
            Some(std::mem::transmute::<
                unsafe extern "C" fn(*mut GObject, *mut GParamSpec, gpointer),
                unsafe extern "C" fn(),
            >(handler)),
            ptr::null_mut(),
            None,
            0,
        );
    }
    let before = NOTIFIED.load(Ordering::SeqCst);

    // SAFETY: a live source.
    unsafe { setting_extension::write::<Stamp>(source.as_ptr(), 42) };

    assert_eq!(NOTIFIED.load(Ordering::SeqCst), before + 1);
}
