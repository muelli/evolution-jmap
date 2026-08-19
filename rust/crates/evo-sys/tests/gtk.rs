// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The GTK surface M7's `insert_widgets` builds its page out of, held against
// the GTK this crate was generated from.
//
// Nothing here constructs a widget, and that is not an oversight. GTK 3 refuses
// to: `gtk_grid_new()` reaches `GtkWidget`'s instance init, which wants a
// `GtkStyleContext`, which aborts the process with "Can't create a
// GtkStyleContext without a display connection" — checked on the machine this
// was written on, which has no display. So the page itself can only be
// exercised in a real Evolution session (M9's Xvfb tier seeds a pre-built
// `.source` file instead of driving it), and this file checks the part that
// *can* be checked without a display, which is also the part that goes wrong
// silently:
//
// - the entry points exist in the libraries this crate links (a missing one is
//   an `undefined symbol` the moment Evolution dlopens the module, i.e. a
//   feature that vanishes with a line in a log nobody reads);
// - the types those entry points take are the classes we believe they are, and
//   stand in the inheritance relations that make the pointer casts between them
//   legitimate — GTK's C API takes a `GtkGrid *` here and a `GtkWidget *` there
//   for the same object, and on this side of the ABI those are separate opaque
//   types precisely so the cast has to be written down;
// - and that the opaque types stayed opaque: a handle with a size is a struct
//   somebody generated a layout for, and a wrong layout for a `GtkWidget` is
//   how you scribble over a live widget.

use evo_sys::*;
use std::mem::size_of;
use std::sync::OnceLock;

/// Every widget class this crate names, with the name GTK registers it under.
const CLASSES: &[(&str, unsafe extern "C" fn() -> GType)] = &[
    ("GtkWidget", gtk_widget_get_type),
    ("GtkContainer", gtk_container_get_type),
    ("GtkBox", gtk_box_get_type),
    ("GtkGrid", gtk_grid_get_type),
    ("GtkLabel", gtk_label_get_type),
    ("GtkEntry", gtk_entry_get_type),
    ("GtkCheckButton", gtk_check_button_get_type),
    ("GtkComboBoxText", gtk_combo_box_text_get_type),
];

/// The classes, registered exactly once — which has to be arranged, because on
/// GTK 3 it is not safe to leave to chance.
///
/// `gtk_container_get_type()` is hand-written rather than a `G_DEFINE_TYPE`
/// macro, and its once-guard is a plain `static GType container_type = 0;`
/// tested and assigned with no `g_once_init_enter` around it. Two threads
/// calling it at the same moment therefore both see zero and both register:
/// caught here first time, as `cannot register existing type 'GtkContainer'`
/// followed by a `<invalid>` type whose `get_type` then returns 0 forever, from
/// two `#[test]`s that touched the type system in parallel — the default test
/// harness runs them on separate threads.
///
/// It is GTK behaving as documented (GTK 3 is to be called from one thread), not
/// a bug to route around, so the tests are what changes: every one of them
/// reaches the type system through here, and `OnceLock` makes the first caller
/// register every class in [`CLASSES`] while the others wait.
fn classes() -> &'static Vec<(&'static str, GType)> {
    static CLASS_TYPES: OnceLock<Vec<(&'static str, GType)>> = OnceLock::new();
    CLASS_TYPES.get_or_init(|| {
        CLASSES
            .iter()
            .map(|(name, get_type)| (*name, unsafe { get_type() }))
            .collect()
    })
}

/// One of the six, by the name this file knows it under.
fn class(wanted: &str) -> GType {
    classes()
        .iter()
        .find(|(name, _)| *name == wanted)
        .map(|(_, gtype)| *gtype)
        .unwrap_or_else(|| panic!("{wanted} is not one of the classes this crate names"))
}

fn type_name(gtype: GType) -> String {
    unsafe {
        let name = g_type_name(gtype);
        assert!(!name.is_null(), "g_type_name returned NULL for {gtype}");
        std::ffi::CStr::from_ptr(name)
            .to_string_lossy()
            .into_owned()
    }
}

/// The classes are the ones we think they are — asserted by name rather than by
/// numeric `GType`, which is an address that differs every run.
#[test]
fn the_widget_types_are_the_classes_this_crate_names() {
    for (expected, gtype) in classes() {
        assert_ne!(
            *gtype, 0,
            "{expected}: get_type() returned an invalid GType"
        );
        assert_eq!(
            type_name(*gtype).as_str(),
            *expected,
            "the type behind {expected}'s getter is registered under another name"
        );
    }
}

/// What licenses the casts. `gtk_grid_new` hands back a `GtkWidget *` that
/// `gtk_grid_attach` must be given as a `GtkGrid *`, and `gtk_grid_attach`'s
/// child argument is a `GtkWidget *` that will be a label or an entry: three
/// casts whose soundness is exactly the statement that GTK's type system relates
/// the classes this way. If GTK ever reparented one of them — GtkLabel's parent
/// really is the deprecated `GtkMisc` on 3.24, for instance — the cast that
/// crosses the changed edge is the one that starts reading the wrong offsets.
#[test]
fn the_casts_the_widget_calls_require_are_upcasts_gtk_agrees_with() {
    let widget = class("GtkWidget");
    let container = class("GtkContainer");
    for (name, gtype) in classes() {
        assert_eq!(
            unsafe { g_type_is_a(*gtype, widget) },
            GTRUE,
            "{name} is no longer a GtkWidget: every call here that takes a GtkWidget * would be handed one that is not"
        );
    }
    for name in ["GtkBox", "GtkGrid"] {
        assert_eq!(
            unsafe { g_type_is_a(class(name), container) },
            GTRUE,
            "{name} is no longer a GtkContainer, so it is no longer a thing children are added to"
        );
    }
}

/// The handles are handles. Nothing in this crate knows what a `GtkWidget`
/// looks like, and this is the assertion that keeps it that way: a non-zero size
/// here means a layout was generated for a GTK class struct, which is the
/// binding surface `build.rs` refuses to take on — and, unlike the EDS structs
/// `tests/layout.rs` checks, one nothing cross-checks against `g_type_query`.
#[test]
fn the_widget_handles_carry_no_layout() {
    assert_eq!(size_of::<GtkWidget>(), 0, "GtkWidget is no longer opaque");
    assert_eq!(size_of::<GtkBox>(), 0, "GtkBox is no longer opaque");
    assert_eq!(size_of::<GtkGrid>(), 0, "GtkGrid is no longer opaque");
    assert_eq!(size_of::<GtkLabel>(), 0, "GtkLabel is no longer opaque");
    assert_eq!(size_of::<GtkEntry>(), 0, "GtkEntry is no longer opaque");
    assert_eq!(
        size_of::<GtkComboBoxText>(),
        0,
        "GtkComboBoxText is no longer opaque"
    );
}

/// Every call the page needs, named once. The load-bearing half of this test is
/// that it *links*: a name that is not in the GTK this crate was built against
/// fails the test binary's link, which is the same failure Evolution would hit
/// as an `undefined symbol` on dlopen, moved to where it is a red test. The
/// assertion itself is a formality that stops the addresses being optimised
/// away.
#[test]
fn every_widget_entry_point_this_crate_offers_resolves() {
    let entry_points: &[(&str, *const ())] = &[
        ("gtk_box_pack_start", gtk_box_pack_start as *const ()),
        ("gtk_grid_new", gtk_grid_new as *const ()),
        ("gtk_grid_attach", gtk_grid_attach as *const ()),
        (
            "gtk_grid_set_row_spacing",
            gtk_grid_set_row_spacing as *const (),
        ),
        (
            "gtk_grid_set_column_spacing",
            gtk_grid_set_column_spacing as *const (),
        ),
        (
            "gtk_label_new_with_mnemonic",
            gtk_label_new_with_mnemonic as *const (),
        ),
        (
            "gtk_label_set_mnemonic_widget",
            gtk_label_set_mnemonic_widget as *const (),
        ),
        ("gtk_label_set_xalign", gtk_label_set_xalign as *const ()),
        ("gtk_entry_new", gtk_entry_new as *const ()),
        (
            "gtk_widget_set_hexpand",
            gtk_widget_set_hexpand as *const (),
        ),
        ("gtk_widget_show_all", gtk_widget_show_all as *const ()),
        (
            "gtk_check_button_new_with_mnemonic",
            gtk_check_button_new_with_mnemonic as *const (),
        ),
        ("gtk_label_new", gtk_label_new as *const ()),
        ("gtk_label_set_text", gtk_label_set_text as *const ()),
        (
            "gtk_widget_set_visible",
            gtk_widget_set_visible as *const (),
        ),
        (
            "gtk_combo_box_text_new",
            gtk_combo_box_text_new as *const (),
        ),
        (
            "gtk_combo_box_text_append",
            gtk_combo_box_text_append as *const (),
        ),
    ];
    for (name, address) in entry_points {
        assert!(!address.is_null(), "{name} resolved to NULL");
    }
}
