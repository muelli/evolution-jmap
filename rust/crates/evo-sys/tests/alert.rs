// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The surface of the info bar that `jmap-ui` shows after a collection delete:
// an `EAlert` with a `GtkButton`, submitted to the content of a shell view.
//
// No test makes a widget, for the reason in `tests/gtk.rs`. These tests check
// the type names, the casts and the entry points.

use std::ffi::CStr;
use std::mem::size_of;
use std::sync::OnceLock;

use evo_sys::*;

fn type_name(gtype: GType) -> String {
    // SAFETY: a registered GType has a static name.
    let name = unsafe { g_type_name(gtype) };
    assert!(!name.is_null(), "g_type_name returned NULL for {gtype}");
    // SAFETY: a NUL-terminated static string.
    unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned()
}

/// The types of the info bar, registered one time.
///
/// GTK 3 registers some of its types with no lock (`tests/gtk.rs` tells the
/// details). Two tests that register `GtkButton` in parallel can fail or hang.
/// Thus each test gets the types from here.
#[derive(Clone, Copy)]
struct Types {
    alert: GType,
    sink: GType,
    button: GType,
    widget: GType,
    content: GType,
}

fn types() -> &'static Types {
    static TYPES: OnceLock<Types> = OnceLock::new();
    // SAFETY: each getter takes no arguments and registers its own type.
    TYPES.get_or_init(|| unsafe {
        Types {
            alert: e_alert_get_type(),
            sink: e_alert_sink_get_type(),
            button: gtk_button_get_type(),
            widget: gtk_widget_get_type(),
            content: e_shell_content_get_type(),
        }
    })
}

#[test]
fn the_alert_types_are_the_classes_that_jmap_ui_names() {
    let Types {
        alert,
        sink,
        button,
        ..
    } = *types();
    assert_eq!(type_name(alert), "EAlert");
    assert_eq!(type_name(sink), "EAlertSink");
    assert_eq!(type_name(button), "GtkButton");
}

/// `jmap-ui` casts the shell content to `EAlertSink *` and the button to
/// `GtkWidget *`. Each cast is correct only if GLib agrees.
#[test]
fn the_casts_of_the_info_bar_are_ones_that_glib_agrees_with() {
    let types = types();
    // SAFETY: two registered types.
    unsafe {
        assert_eq!(
            g_type_is_a(types.content, types.sink),
            GTRUE,
            "EShellContent is no longer an EAlertSink"
        );
        assert_eq!(
            g_type_is_a(types.button, types.widget),
            GTRUE,
            "GtkButton is no longer a GtkWidget"
        );
    }
}

#[test]
fn the_alert_handles_carry_no_layout() {
    assert_eq!(size_of::<EAlert>(), 0, "EAlert is no longer opaque");
    assert_eq!(size_of::<EAlertSink>(), 0, "EAlertSink is no longer opaque");
    assert_eq!(size_of::<GtkButton>(), 0, "GtkButton is no longer opaque");
}

/// A link failure here is an `undefined symbol` when Evolution loads the module.
#[test]
fn every_entry_point_of_the_info_bar_resolves() {
    let entry_points: &[(&str, *const ())] = &[
        ("e_alert_new", e_alert_new as *const ()),
        (
            "e_alert_set_primary_text",
            e_alert_set_primary_text as *const (),
        ),
        ("e_alert_add_widget", e_alert_add_widget as *const ()),
        ("e_alert_start_timer", e_alert_start_timer as *const ()),
        ("e_alert_response", e_alert_response as *const ()),
        (
            "e_alert_sink_submit_alert",
            e_alert_sink_submit_alert as *const (),
        ),
        (
            "gtk_button_new_with_mnemonic",
            gtk_button_new_with_mnemonic as *const (),
        ),
        ("gtk_widget_show", gtk_widget_show as *const ()),
    ];
    for (name, address) in entry_points {
        assert!(!address.is_null(), "{name} resolved to NULL");
    }
}
