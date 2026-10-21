// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The two `ESourceConfigBackend` subclasses, and the class fields that are the
// only reason for them.
//
// The operator used real Evolution with a real JMAP account and could not
// create a JMAP address book or calendar. The New Address Book and New Calendar
// dialogs offered only On This Computer, CardDAV, Google, LDAP and Weather. The
// properties dialog of an existing JMAP calendar showed empty `Type:` and
// `Name:` labels, a black colour picker, and an OK button that did nothing. All
// four have one cause. `ESourceConfig` builds one candidate for each registered
// `ESourceConfigBackend`, and this project registered none. Thus
// `source_config_get_active_candidate` had nothing to return, and each later
// call asserted on the NULL.
//
// A headless test can examine the class, which holds all of the identity of a
// candidate. `g_type_class_ref` runs `class_init` and needs no instance. A test
// can examine nothing more. An `ESourceConfigBackend` is an `EExtension` of an
// `ESourceConfig`, which is a `GtkBox`. Thus no instance can exist without a
// display, and this VM has none. For this reason, the `allow_creation` decision
// is a plain Rust function over the source type, and the vfunc is a short
// wrapper around it. A test can thus reach the part that can be wrong.

use std::ffi::CStr;
use std::ptr;

use eds_sys::EExtensionClass;
use evo_sys::{
    E_CAL_CLIENT_SOURCE_TYPE_EVENTS, E_CAL_CLIENT_SOURCE_TYPE_MEMOS,
    E_CAL_CLIENT_SOURCE_TYPE_TASKS, ESourceConfigBackendClass, e_book_source_config_get_type,
    e_cal_source_config_get_type, e_source_config_backend_get_type, e_source_config_get_type,
};
use glib_sys::GType;
use gobject_sys::{g_type_class_ref, g_type_class_unref, g_type_parent};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::subclass::{ObjectSubclass, register_static};
use jmap_config::source_config::{
    BACKEND_NAME, JmapBookConfig, JmapCalConfig, config_backend_type, offers_creation_for,
};

// Initialise the parent classes of the Evolution types before the tests start threads.
jmap_backend_core::initialise_parents_before_main!(
    jmap_config::backend::JmapConfigServiceBackend,
    jmap_config::config_lookup::JmapConfigLookup,
    jmap_config::source_config::JmapBookConfig,
    jmap_config::source_config::JmapCalConfig,
);

/// A registered class. The test keeps a reference, as in `tests/backend.rs`,
/// for the same reason: `class_init` runs on the first `g_type_class_ref`, and
/// the struct is readable only while a reference is held.
struct Class(*mut ESourceConfigBackendClass);

impl Class {
    fn of<T: ObjectSubclass>() -> Self {
        let gtype = register_static::<T>();
        assert_ne!(gtype, 0, "{:?} did not register", T::NAME);
        // SAFETY: the type is registered, so a reference to its class runs
        // class_init. The class struct of this crate starts with
        // ESourceConfigBackendClass.
        Self(unsafe { g_type_class_ref(gtype) }.cast())
    }

    fn vfuncs(&self) -> &ESourceConfigBackendClass {
        // SAFETY: the class stays referenced while `self` is alive.
        unsafe { &*self.0 }
    }

    /// The part of the same struct that Evolution's `EExtension` owns. It holds
    /// `extensible_type`.
    fn extension(&self) -> &EExtensionClass {
        // SAFETY: ESourceConfigBackendClass starts with EExtensionClass.
        // `evo-sys/tests/source_config.rs` pins this against `g_type_query`.
        unsafe { &*ptr::from_ref(self.vfuncs()).cast::<EExtensionClass>() }
    }
}

impl Drop for Class {
    fn drop(&mut self) {
        // SAFETY: this releases the reference from `of` exactly one time.
        unsafe { g_type_class_unref(self.0.cast()) };
    }
}

/// Evolution's own class, for the comparisons below ("did `class_init` replace
/// this slot or inherit it?").
fn parent_class() -> &'static ESourceConfigBackendClass {
    // SAFETY: this references the class of a registered type and never
    // releases the reference, as `'static` requires. It uses
    // `config_backend_type`, not the bare accessor. This reference can be the
    // one that runs Evolution's class initialiser, and that initialiser must
    // find `ESourceConfig` already registered.
    let class = unsafe { g_type_class_ref(config_backend_type()) };
    assert!(!class.is_null(), "g_type_class_ref returned NULL");
    // SAFETY: the reference above is never released.
    unsafe { &*class.cast::<ESourceConfigBackendClass>() }
}

/// Both types must be children of the class that `ESourceConfig` examines. It
/// does not find a backend by name. `e_extensible_load_extensions` makes an
/// instance of each child of `EExtension` with a matching `extensible_type`.
/// `source_config_init_backends` then keeps the `ESourceConfigBackend`s.
#[test]
fn both_types_extend_the_class_the_dialogs_build_candidates_from() {
    for (name, gtype) in [
        ("EBookConfigJmap", register_static::<JmapBookConfig>()),
        ("ECalConfigJmap", register_static::<JmapCalConfig>()),
    ] {
        assert_ne!(gtype, 0, "{name} did not register");
        assert_eq!(
            // SAFETY: both are registered types.
            unsafe { g_type_parent(gtype) },
            unsafe { e_source_config_backend_get_type() },
            "{name} does not derive from ESourceConfigBackend"
        );
    }
}

/// The field that selects *the dialog* that shows a candidate. Apart from
/// `allow_creation`, this is the one difference between the two subclasses.
///
/// `extensible_type` comes from `ESourceConfigBackendClass`, where Evolution's
/// own `class_init` sets it to `E_TYPE_SOURCE_CONFIG`, the base class. That
/// value is the silent failure. The backend then gets an instance for *each*
/// `ESourceConfig`. The address book subclass gets the scratch source of a
/// calendar, and the opposite. Both offer a "JMAP" entry in the dialog of the
/// other. Evolution's own modules always name the derived class, and this test
/// makes sure that the modules of this crate do too.
#[test]
fn each_backend_extends_exactly_the_dialog_it_is_for() {
    let book = Class::of::<JmapBookConfig>();
    let cal = Class::of::<JmapCalConfig>();

    assert_eq!(
        book.extension().extensible_type,
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_book_source_config_get_type() },
        "the address book backend does not extend EBookSourceConfig, so New \
         Address Book would never build a candidate for it"
    );
    assert_eq!(
        cal.extension().extensible_type,
        // SAFETY: as above.
        unsafe { e_cal_source_config_get_type() },
        "the calendar backend does not extend ECalSourceConfig, so New \
         Calendar would never build a candidate for it"
    );
    assert_ne!(
        book.extension().extensible_type,
        cal.extension().extensible_type,
        "both backends extend the same dialog class"
    );
    for (name, class) in [("address book", &book), ("calendar", &cal)] {
        assert_ne!(
            class.extension().extensible_type,
            // SAFETY: as above.
            unsafe { e_source_config_get_type() },
            "the {name} backend still carries the inherited ESourceConfig, so \
             it would be instantiated for every source dialog"
        );
    }
}

/// A candidate must match `backend_name` on both paths into
/// `source_config_init_candidates`. To *create*, the dialog looks up the
/// `[Collection] BackendName` of each eligible collection source in its backend
/// table. To *edit*, it looks up the `[Address Book]`/`[Calendar] BackendName`
/// of the existing source. These are two different strings in the registry, and
/// both are `"jmap"`. Thus one value serves both. It must be the value that
/// `jmap-collection-sync` writes onto each child it discovers. If not, an
/// existing JMAP address book still opens for edit with a NULL backend.
#[test]
fn both_backends_answer_to_the_name_a_jmap_child_source_carries() {
    assert_eq!(
        BACKEND_NAME.to_str().ok(),
        Some(jmap_collection_sync::child_source::BACKEND_NAME),
        "the config backends answer to a different name than the one written \
         onto a discovered child source"
    );

    for (name, class) in [
        ("address book", Class::of::<JmapBookConfig>()),
        ("calendar", Class::of::<JmapCalConfig>()),
    ] {
        // SAFETY: the 'static NUL-terminated string from class initialisation,
        // or NULL, for which `read_string` answers `None`.
        let carried = unsafe { read_string(class.vfuncs().backend_name) };
        assert_eq!(
            carried.as_deref(),
            BACKEND_NAME.to_str().ok(),
            "the {name} backend names no provider, or the wrong one"
        );
    }
}

/// `parent_uid` names a *fixed* parent source: Evolution's "local-stub" and
/// similar placeholders, below which the On This Computer address books are.
/// A non-NULL value sends the backend through the first loop of
/// `init_candidates`. That loop builds one scratch source with that fixed uid as
/// parent, and never examines a collection.
///
/// A JMAP address book or calendar is below the account that discovered it.
/// Thus NULL puts this backend in the *second* loop, where the parent is a
/// collection source from `list_eligible_collections`. A wrong value does not
/// cause a crash. It causes a `g_warning` about an invalid parent_uid and a
/// `continue`: the original symptom, with a line in the journal.
#[test]
fn neither_backend_claims_a_fixed_parent_source() {
    for (name, class) in [
        ("address book", Class::of::<JmapBookConfig>()),
        ("calendar", Class::of::<JmapCalConfig>()),
    ] {
        assert!(
            class.vfuncs().parent_uid.is_null(),
            "the {name} backend claims a fixed parent uid, so its candidate \
             would be parented to a stub source instead of to the JMAP account"
        );
    }
}

/// Each of the four slots dispatches through a `g_return_if_fail (class->
/// <vfunc> != NULL)`. Thus a NULL gives a CRITICAL and a dialog that does
/// nothing. The slots are non-NULL because Evolution's own `class_init` fills
/// all four with defaults, and GObject copies the parent class into the
/// subclass before `class_init` runs. This test makes sure that the `class_init`
/// of this crate did not clear one. That is the only possible failure here.
#[test]
fn all_four_slots_are_filled_after_class_init() {
    for (name, class) in [
        ("address book", Class::of::<JmapBookConfig>()),
        ("calendar", Class::of::<JmapCalConfig>()),
    ] {
        let vfuncs = class.vfuncs();
        assert!(
            vfuncs.allow_creation.is_some(),
            "{name}: no allow_creation — e_source_config_backend_allow_creation \
             returns FALSE on the NULL, so the type never appears in the list"
        );
        assert!(
            vfuncs.insert_widgets.is_some(),
            "{name}: no insert_widgets — the candidate's page is never built"
        );
        assert!(
            vfuncs.check_complete.is_some(),
            "{name}: no check_complete — OK stays insensitive forever"
        );
        assert!(
            vfuncs.commit_changes.is_some(),
            "{name}: no commit_changes — pressing OK saves nothing"
        );
    }
}

/// Three of the four slots are deliberately *not* overridden. That is a
/// decision, not an omission, so a test makes sure of it.
///
/// Evolution's defaults are, in sequence: insert no widgets, answer complete,
/// and commit nothing. Each is correct here. The user must give only the name
/// and colour, which `ESourceConfig` shows itself. A JMAP address book needs no
/// URL, because its account already knows the server. `ESourceConfig`'s own
/// `check_complete` already refuses an empty Name and an unselected Type. There
/// is nothing to commit: when the scratch source is written into the registry,
/// `evolution-source-registry` calls the `create_resource_sync` of this account.
/// A `commit_changes` of this crate would be a second writer that competes.
#[test]
fn the_three_slots_with_nothing_to_add_stay_evolutions_own() {
    let inherited = parent_class();
    for (name, class) in [
        ("address book", Class::of::<JmapBookConfig>()),
        ("calendar", Class::of::<JmapCalConfig>()),
    ] {
        let vfuncs = class.vfuncs();
        for (slot, ours, theirs) in [
            (
                "insert_widgets",
                vfuncs.insert_widgets.map(|f| f as usize),
                inherited.insert_widgets.map(|f| f as usize),
            ),
            (
                "check_complete",
                vfuncs.check_complete.map(|f| f as usize),
                inherited.check_complete.map(|f| f as usize),
            ),
            (
                "commit_changes",
                vfuncs.commit_changes.map(|f| f as usize),
                inherited.commit_changes.map(|f| f as usize),
            ),
        ] {
            assert_eq!(
                ours, theirs,
                "{name}: {slot} is no longer Evolution's own — if that is \
                 deliberate, this test is the place to say why"
            );
        }
    }
}

/// The one slot that *is* overridden, and only on the calendar side.
///
/// One `ECalSourceConfig` class serves New Calendar, New Task List and New Memo
/// List. Only `e_cal_source_config_get_source_type` makes them different. Thus
/// a calendar config backend that inherits `allow_creation` (which always
/// answers TRUE) is in all three. Evolution's own `cal-config-google` overrides
/// it for this reason.
#[test]
fn only_the_calendar_backend_narrows_what_it_will_be_offered_for() {
    let inherited = parent_class().allow_creation.map(|f| f as usize);

    assert_eq!(
        Class::of::<JmapBookConfig>()
            .vfuncs()
            .allow_creation
            .map(|f| f as usize),
        inherited,
        "the address book backend overrode allow_creation — there is only one \
         kind of address book dialog, so there is nothing to narrow"
    );
    assert_ne!(
        Class::of::<JmapCalConfig>()
            .vfuncs()
            .allow_creation
            .map(|f| f as usize),
        inherited,
        "the calendar backend inherited allow_creation, so JMAP would be \
         offered in New Task List and New Memo List too"
    );
}

/// All of `allow_creation`, as a function of the value that the vfunc gets.
/// This is the part that can be wrong. It is a separate function because the
/// fetch needs an `ECalSourceConfig`, and thus a display.
///
/// A JMAP account has calendars. RFC 8984 JSCalendar models tasks, but this
/// project registers no task-list or memo-list backend factory. Thus a source
/// committed from New Task List names a `[Task List] BackendName=jmap` that
/// nothing can open. Before that, `create_resource_sync` refuses it, because
/// `requested_of` answers `None` for a source with neither the
/// `[Address Book]` nor the `[Calendar]` extension.
#[test]
fn only_an_events_calendar_can_be_created_over_jmap() {
    assert!(
        offers_creation_for(E_CAL_CLIENT_SOURCE_TYPE_EVENTS),
        "New Calendar would not offer JMAP"
    );
    assert!(
        !offers_creation_for(E_CAL_CLIENT_SOURCE_TYPE_TASKS),
        "New Task List would offer JMAP, and there is no JMAP task list backend"
    );
    assert!(
        !offers_creation_for(E_CAL_CLIENT_SOURCE_TYPE_MEMOS),
        "New Memo List would offer JMAP, and there is no JMAP memo list backend"
    );
}

/// The GLib names of the two types. A `g_warning` from `init_candidates`
/// prints these strings, and a second registration collides on them. Evolution
/// names a module of this shape `E<Book|Cal>Config<Provider>`, and the names of
/// this crate do the same.
#[test]
fn the_registered_names_follow_evolutions_own() {
    assert_eq!(
        <JmapBookConfig as ObjectSubclass>::NAME,
        c"EBookConfigJmap" as &CStr
    );
    assert_eq!(
        <JmapCalConfig as ObjectSubclass>::NAME,
        c"ECalConfigJmap" as &CStr
    );
}

/// The registration-sequence hazard that `ObjectSubclass::class_init_types`
/// prevents, in a different library. Both `class_init`s ask Evolution for a
/// `GType` that is registered on the first request. `e_book_source_config_get_type`
/// and `e_cal_source_config_get_type` each register a `GtkBox` subclass. That
/// call from inside a class initialiser caused a deadlock in `evo-sys`'s own
/// `tests/source_config.rs` before it had `warm_up()`.
///
/// The fix is to declare the types. This test makes sure that each declaration
/// names the type that its `class_init` requests. `register` calls
/// `class_init_types()` before it registers the declaring type, on a thread
/// that holds neither lock. Thus the `g_once` of the accessor completes there,
/// and the request in `class_init` uses its fast path. A declaration of the
/// *other* dialog class also gives `len() == 1`, but the hazard stays. Thus the
/// test compares the type identity.
#[test]
fn each_type_declares_the_dialog_class_its_class_init_asks_for() {
    for (name, declared, wanted) in [
        (
            "EBookConfigJmap",
            JmapBookConfig::class_init_types(),
            // SAFETY: no arguments, and the type initialises itself.
            unsafe { e_book_source_config_get_type() },
        ),
        (
            "ECalConfigJmap",
            JmapCalConfig::class_init_types(),
            // SAFETY: as above.
            unsafe { e_cal_source_config_get_type() },
        ),
    ] {
        assert_ne!(wanted, 0, "{name}'s dialog class did not register");
        let declared: Vec<GType> = declared;
        assert_eq!(
            declared,
            vec![wanted],
            "{name} does not declare the dialog class its class_init asks for, \
             so that ask is still the one that registers it — from inside \
             GLib's class-init lock"
        );
    }
}
