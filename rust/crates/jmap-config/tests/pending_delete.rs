// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests the `[JMAP Pending Delete]` extension on real `ESource`s, through the
// `read`/`write` functions of this crate and through the raw GObject property
// that the EDS `.source` (de)serialisation uses.

use std::ffi::{CStr, c_void};
use std::ptr;
use std::sync::Mutex;

use eds_sys::{
    e_source_get_extension, e_source_has_extension, e_source_parameter_to_key, e_source_to_string,
};
use glib_sys::{GFALSE, g_free};
use gobject_sys::{
    G_TYPE_INT64, GObject, GParamSpec, GValue, g_object_get_property, g_object_set_property,
    g_signal_connect_data, g_value_get_int64, g_value_init, g_value_set_int64, g_value_unset,
};
use jmap_backend_core::callback::gcallback;
use jmap_backend_core::source::OwnedSource;
use jmap_collection_sync::pending::Due;
use jmap_config::pending_delete::{EXTENSION_NAME, PROPERTY_DUE, ensure_registered, read, write};

struct TestSource(OwnedSource);

impl TestSource {
    fn new() -> Self {
        // The raw property helpers below do not register the type, so the
        // test registers it, as the module load does.
        ensure_registered();
        Self(OwnedSource::new("jmap-child"))
    }

    fn due(&self) -> Option<Due> {
        // SAFETY: a live source.
        unsafe { read(self.0.as_ptr()) }
    }

    fn write(&self, due: Option<Due>) {
        // SAFETY: a live source.
        unsafe { write(self.0.as_ptr(), due) };
    }

    fn has_extension(&self) -> bool {
        // SAFETY: a live source.
        unsafe { e_source_has_extension(self.0.as_ptr(), EXTENSION_NAME.as_ptr()) != GFALSE }
    }

    fn extension(&self) -> *mut GObject {
        // SAFETY: a live source, and the type is registered.
        unsafe { e_source_get_extension(self.0.as_ptr(), EXTENSION_NAME.as_ptr()).cast() }
    }

    /// Reads `due` through the raw GObject API, as the EDS keyfile writer does.
    fn raw_due(&self) -> i64 {
        // SAFETY: a live extension with an int64 `due` property, and a GValue
        // that is initialised before the read and unset after it.
        unsafe {
            let mut value: GValue = std::mem::zeroed();
            g_value_init(&mut value, G_TYPE_INT64);
            g_object_get_property(self.extension(), PROPERTY_DUE.as_ptr(), &mut value);
            let stamp = g_value_get_int64(&value);
            g_value_unset(&mut value);
            stamp
        }
    }

    /// Writes `due` through the raw GObject API, as the EDS keyfile parser does.
    fn set_raw_due(&self, stamp: i64) {
        // SAFETY: as in `raw_due`.
        unsafe {
            let mut value: GValue = std::mem::zeroed();
            g_value_init(&mut value, G_TYPE_INT64);
            g_value_set_int64(&mut value, stamp);
            g_object_set_property(self.extension(), PROPERTY_DUE.as_ptr(), &value);
            g_value_unset(&mut value);
        }
    }

    /// The source as EDS writes it to disk.
    fn serialised(&self) -> String {
        // SAFETY: a live source. The result is `(transfer full)`.
        unsafe {
            let text = e_source_to_string(self.0.as_ptr(), ptr::null_mut());
            assert!(!text.is_null(), "e_source_to_string returned NULL");
            let owned = CStr::from_ptr(text).to_string_lossy().into_owned();
            g_free(text.cast());
            owned
        }
    }
}

const STAMP: i64 = 1_792_000_000;

fn due() -> Due {
    Due::from_stamp(STAMP).expect("a positive stamp")
}

#[test]
fn a_source_without_the_extension_has_no_pending_delete() {
    let source = TestSource::new();
    assert_eq!(source.due(), None);
}

#[test]
fn a_read_does_not_add_the_extension() {
    let source = TestSource::new();
    source.due();
    assert!(!source.has_extension());
}

#[test]
fn clearing_a_source_without_a_pending_delete_does_not_add_the_extension() {
    let source = TestSource::new();
    source.write(None);
    assert!(!source.has_extension());
}

#[test]
fn a_written_due_time_is_read_back() {
    let source = TestSource::new();
    source.write(Some(due()));
    assert_eq!(source.due(), Some(due()));
}

#[test]
fn a_cleared_due_time_reads_as_no_pending_delete() {
    let source = TestSource::new();
    source.write(Some(due()));
    source.write(None);
    assert_eq!(source.due(), None);
    assert_eq!(source.raw_due(), 0);
}

#[test]
fn the_write_goes_through_the_gobject_property() {
    let source = TestSource::new();
    source.write(Some(due()));
    assert_eq!(source.raw_due(), STAMP);
}

#[test]
fn a_value_that_the_keyfile_parser_sets_is_read_back() {
    let source = TestSource::new();
    source.set_raw_due(STAMP);
    assert_eq!(source.due(), Some(due()));
}

#[test]
fn a_negative_stamp_reads_as_no_pending_delete() {
    let source = TestSource::new();
    source.set_raw_due(-5);
    assert_eq!(source.due(), None);
}

#[test]
fn the_due_time_persists_into_the_keyfile() {
    let source = TestSource::new();
    source.write(Some(due()));
    let text = source.serialised();

    let group = format!("[{}]", EXTENSION_NAME.to_string_lossy());
    assert!(text.contains(&group), "no {group} group:\n{text}");
    // SAFETY: a NUL-terminated property name. The result is `(transfer full)`.
    let key = unsafe {
        let raw = e_source_parameter_to_key(PROPERTY_DUE.as_ptr());
        let key = CStr::from_ptr(raw).to_string_lossy().into_owned();
        g_free(raw.cast());
        key
    };
    assert!(
        text.contains(&format!("{key}={STAMP}")),
        "{key} did not persist:\n{text}"
    );
}

static NOTIFIED: Mutex<Vec<String>> = Mutex::new(Vec::new());

unsafe extern "C" fn record_notify(_object: *mut GObject, pspec: *mut GParamSpec, _: *mut c_void) {
    // SAFETY: GObject passes a live pspec with a static name.
    let name = unsafe { CStr::from_ptr((*pspec).name) };
    NOTIFIED
        .lock()
        .unwrap()
        .push(name.to_string_lossy().into_owned());
}

#[test]
fn a_write_raises_notify_so_the_registry_sends_the_change() {
    let source = TestSource::new();
    source.write(Some(due()));
    let handler: unsafe extern "C" fn(*mut GObject, *mut GParamSpec, *mut c_void) = record_notify;
    // SAFETY: the handler has the signature of `notify`, and needs no data.
    unsafe {
        g_signal_connect_data(
            source.extension(),
            c"notify".as_ptr(),
            gcallback(handler),
            ptr::null_mut(),
            None,
            0,
        );
    }
    source.write(None);
    assert!(
        NOTIFIED
            .lock()
            .unwrap()
            .iter()
            .any(|name| name.as_str() == PROPERTY_DUE.to_str().unwrap()),
        "a write raised no notify for {PROPERTY_DUE:?}"
    );
}
