// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The two `ESourceConfigBackend` subclasses, and the class fields that are the
// entire reason they exist.
//
// The operator drove real Evolution against a real JMAP account and could not
// create a JMAP address book or calendar at all — the New Address Book and New
// Calendar dialogs offered only On This Computer, CardDAV, Google, LDAP and
// Weather — while an existing JMAP calendar's properties dialog showed empty
// `Type:` and `Name:` labels, a colour picker reading black, and an OK button
// that did nothing. All four are one thing: `ESourceConfig` builds one
// candidate per registered `ESourceConfigBackend`, and this project registered
// none, so `source_config_get_active_candidate` had nothing to return and every
// later call asserted on the NULL.
//
// What a headless test can hold is the class, which is where the whole of a
// candidate's identity lives — `g_type_class_ref` runs `class_init` and needs
// no instance. What it cannot hold is anything past that: an
// `ESourceConfigBackend` is an `EExtension` of an `ESourceConfig`, which is a
// `GtkBox`, so no instance can exist without a display this VM does not have.
// That is why `allow_creation`'s decision is a plain Rust function over the
// source type and the vfunc is a three-line wrapper around it: the part with a
// wrong answer available is the part a test can reach.

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
    BACKEND_NAME, JmapBookConfig, JmapCalConfig, offers_creation_for,
};

/// A registered class, kept referenced for the test's duration — the same
/// shape `tests/backend.rs` uses, and for the same reason: `class_init` runs on
/// the first `g_type_class_ref` and the struct is only readable while a
/// reference is held.
struct Class(*mut ESourceConfigBackendClass);

impl Class {
    fn of<T: ObjectSubclass>() -> Self {
        let gtype = register_static::<T>();
        assert_ne!(gtype, 0, "{:?} did not register", T::NAME);
        // SAFETY: the type is registered, so referencing its class runs
        // class_init; our class struct leads with ESourceConfigBackendClass.
        Self(unsafe { g_type_class_ref(gtype) }.cast())
    }

    fn vfuncs(&self) -> &ESourceConfigBackendClass {
        // SAFETY: the class is referenced for as long as `self` lives.
        unsafe { &*self.0 }
    }

    /// The half of the same struct that Evolution's `EExtension` owns, where
    /// `extensible_type` lives.
    fn extension(&self) -> &EExtensionClass {
        // SAFETY: ESourceConfigBackendClass leads with EExtensionClass, which
        // `evo-sys/tests/source_config.rs` pins against `g_type_query`.
        unsafe { &*ptr::from_ref(self.vfuncs()).cast::<EExtensionClass>() }
    }
}

impl Drop for Class {
    fn drop(&mut self) {
        // SAFETY: the reference taken in `of` is given back exactly once.
        unsafe { g_type_class_unref(self.0.cast()) };
    }
}

/// Evolution's own class, for the "did `class_init` displace this slot or
/// inherit it?" comparisons below.
fn parent_class() -> &'static ESourceConfigBackendClass {
    // SAFETY: no arguments; referencing the class of a registered type and
    // never giving the reference back, which is what `'static` claims.
    let class = unsafe { g_type_class_ref(e_source_config_backend_get_type()) };
    assert!(!class.is_null(), "g_type_class_ref returned NULL");
    // SAFETY: the reference above is never released.
    unsafe { &*class.cast::<ESourceConfigBackendClass>() }
}

/// Both types have to be children of the class `ESourceConfig` walks. It does
/// not look a backend up by name: `e_extensible_load_extensions` instantiates
/// every child of `EExtension` whose `extensible_type` matches, and
/// `source_config_init_backends` then keeps the ones that are
/// `ESourceConfigBackend`s.
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

/// The field that decides *which dialog* a candidate turns up in, and the one
/// place the two subclasses differ other than `allow_creation`.
///
/// `extensible_type` is inherited from `ESourceConfigBackendClass`, where
/// Evolution's own `class_init` sets it to `E_TYPE_SOURCE_CONFIG` — the base
/// class. Leaving it at that value is the quiet failure: the backend would be
/// instantiated for *every* `ESourceConfig`, so the address book subclass would
/// be handed a calendar's scratch source and vice versa, and both would offer a
/// "JMAP" entry in the other's dialog. Evolution's own modules always name the
/// derived class, and the assertion is that ours do too.
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

/// `backend_name` is what a candidate is matched against, on both of the two
/// routes into `source_config_init_candidates`: for *creating*, the
/// `[Collection] BackendName` of each eligible collection source is looked up
/// in the dialog's backend table; for *editing*, the existing source's own
/// `[Address Book]`/`[Calendar] BackendName` is. Those are two different
/// strings in the registry and they are both `"jmap"`, so one value serves —
/// and it has to be exactly the one `jmap-collection-sync` writes onto every
/// child it discovers, or an existing JMAP address book still edits to a NULL
/// backend.
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
        // SAFETY: a 'static NUL-terminated string the class was initialised
        // from, or NULL, which `read_string` answers `None` for.
        let carried = unsafe { read_string(class.vfuncs().backend_name) };
        assert_eq!(
            carried.as_deref(),
            BACKEND_NAME.to_str().ok(),
            "the {name} backend names no provider, or the wrong one"
        );
    }
}

/// `parent_uid` names a *fixed* parent source — Evolution's "local-stub" and
/// friends, the placeholders under which On This Computer's address books hang.
/// A non-NULL value here sends the backend down `init_candidates`' first loop,
/// which builds one scratch source parented to that fixed uid and never looks
/// at a collection at all.
///
/// A JMAP address book or calendar hangs off the account that discovered it, so
/// NULL is what puts this backend in the *second* loop, where the parent is one
/// of `list_eligible_collections`' collection sources. Getting it wrong is not
/// a crash: it is a `g_warning` about an invalid parent_uid and a `continue`,
/// i.e. the original symptom back again with a line in the journal.
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

/// Every one of the four slots dispatches through a `g_return_if_fail (class->
/// <vfunc> != NULL)`, so a NULL is a CRITICAL and a dialog that does nothing.
/// They are non-NULL here because Evolution's own `class_init` fills all four
/// with defaults and GObject copies the parent class into ours before
/// `class_init` runs — the assertion is that ours did not clear one, which is
/// the only way this can go wrong from here.
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

/// Three of the four slots are deliberately *not* overridden, and that is a
/// decision rather than an omission, so it is asserted rather than left to be
/// read off the source.
///
/// Evolution's defaults are, in order: insert no widgets, answer complete, and
/// commit nothing. Each is right here. There is nothing to ask the user beyond
/// the name and colour `ESourceConfig` puts on screen itself — a JMAP address
/// book needs no URL, because the account it hangs off already knows the
/// server. Completeness is `ESourceConfig`'s own `check_complete`, which
/// already refuses an empty Name and an unselected Type. And there is nothing
/// to commit: writing the scratch source into the registry is what makes
/// `evolution-source-registry` call this account's `create_resource_sync`, so
/// a `commit_changes` of our own would be a second, competing writer.
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
/// List; they differ by `e_cal_source_config_get_source_type` alone. So a
/// calendar config backend that inherits `allow_creation` — which answers TRUE
/// unconditionally — offers itself in all three. Evolution's own
/// `cal-config-google` overrides it for exactly this reason.
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

/// `allow_creation`'s whole content, as a function of the value the vfunc
/// fetches — the part with a wrong answer available, split out because the
/// fetch itself needs an `ECalSourceConfig` and so a display.
///
/// A JMAP account has calendars. RFC 8984 JSCalendar models tasks, but this
/// project registers no task-list or memo-list backend factory, so a source
/// committed from New Task List would name a `[Task List] BackendName=jmap`
/// nothing can open — and `create_resource_sync` would refuse it first, since
/// `requested_of` answers `None` for a source carrying neither the
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

/// The names GLib knows the two types by, which are the strings a `g_warning`
/// from `init_candidates` prints and the ones a second registration would
/// collide on. Evolution's own spelling for a module of this shape is
/// `E<Book|Cal>Config<Provider>`; ours follow it.
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

/// The registration-order hazard `ObjectSubclass::class_init_types` exists for,
/// one library over: both `class_init`s ask Evolution for a `GType` that is
/// registered on first ask, and `e_book_source_config_get_type` /
/// `e_cal_source_config_get_type` each register a `GtkBox` subclass. Doing that
/// from inside a class initialiser is what deadlocked `evo-sys`'s own
/// `tests/source_config.rs` before its `warm_up()` was written.
///
/// Declaring them is the fix, and this is what says each declaration names the
/// type its own `class_init` actually asks for: `register` calls
/// `class_init_types()` before it registers the declaring type, on a thread
/// holding neither lock, so the accessor's `g_once` completes there and the ask
/// inside `class_init` takes its fast path. A declaration of the *other*
/// dialog class would satisfy `len() == 1` and leave the hazard exactly where
/// it was, so the identity is what is asserted.
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
