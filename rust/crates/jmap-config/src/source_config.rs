// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The two `ESourceConfigBackend` subclasses that add JMAP to Evolution's *New
//! Address Book* and *New Calendar* dialogs.
//!
//! ## What was broken
//!
//! Neither dialog showed JMAP. The properties of an existing JMAP calendar
//! showed an empty `Type:` and an empty `Name:`. The colour picker showed black
//! for a calendar that Evolution drew blue, and the OK button did nothing. The
//! journal showed one cause four times:
//!
//! ```text
//! source_config_init_for_editing_source: assertion 'backend != NULL' failed
//! source_config_get_active_candidate:    assertion 'index >= 0' failed
//! e_source_config_check_complete:        assertion 'candidate != NULL' failed
//! e_source_config_commit:                assertion 'candidate != NULL' failed
//! ```
//!
//! `ESourceConfig` makes one *candidate* (one entry in the Type list, one page of
//! widgets, one commit target) for each registered `ESourceConfigBackend`, and
//! this project registered none. `module-jmap-configuration` had two types: an
//! `EMailConfigServiceBackend` for mail account setup and an
//! `EConfigLookupWorker` for discovery. Neither is an `ESourceConfigBackend`.
//!
//! ## What a subclass must supply, and the source of each value
//!
//! Almost all of it is class fields, not code. Thus this module is short and its
//! test file is long. Evolution's own `module-book-config-local` has the same
//! shape: a `class_init` and nothing more. The source for these facts is
//! `evolution-3.52.4/src/modules/` and `src/e-util/e-source-config.c`, not memory.
//!
//! - **`extensible_type`** selects the dialog that shows the candidate. The
//!   `class_init` of `ESourceConfigBackend` sets it to `E_TYPE_SOURCE_CONFIG`,
//!   the base class. If inherited, both subclasses are in *both* dialogs, and
//!   each sometimes gets the scratch source of the other. Thus the address book
//!   subclass names `E_TYPE_BOOK_SOURCE_CONFIG` and the calendar subclass names
//!   `E_TYPE_CAL_SOURCE_CONFIG`, as Evolution's own modules do.
//! - **`backend_name`** is the value that a candidate must match. The two paths
//!   into `source_config_init_candidates` compare it with two different strings.
//!   Create: the dialog's backend table gets a lookup for the
//!   `[Collection] BackendName` of each source that `list_eligible_collections`
//!   returns. Edit: the lookup uses the existing source's own
//!   `[Address Book]`/`[Calendar] BackendName`. Both are `"jmap"` here:
//!   [`crate::account`] writes the first, `jmap_collection_sync::child_source`
//!   the second. Thus one value serves both, and `tests/source_config.rs` pins
//!   [`BACKEND_NAME`] to the second.
//! - **`parent_uid`** stays NULL. This sends these backends through the
//!   *collection* half of `init_candidates`, not the fixed-parent half. A JMAP
//!   address book is below the account that discovered it, not below one of
//!   Evolution's `local-stub` placeholders.
//! - **`allow_creation`** is overridden on the calendar side only, and
//!   [`offers_creation_for`] tells why.
//! - **`insert_widgets`, `check_complete` and `commit_changes`** are
//!   deliberately Evolution's own. The dialog already shows the name and colour,
//!   and nothing more is necessary. The account knows the server, so, unlike
//!   CardDAV, the user types no URL. `ESourceConfig`'s own `check_complete`
//!   already refuses an empty Name and an unselected Type. Nothing more needs a
//!   commit: when the scratch source is written into the registry,
//!   `e-server-side-source.c` calls this account's `create_resource_sync`. A
//!   `commit_changes` of this module would be a second writer that races the
//!   one that does the work.
//!
//! ## What this connects, and its limits
//!
//! [`jmap_backend_collection::create_resource`](../../jmap_backend_collection/create_resource/index.html)
//! and [`delete_resource`](../../jmap_backend_collection/delete_resource/index.html) were
//! written for D1 but the GUI never called them: nothing could put a scratch
//! source below a JMAP account for the registry to process. The same applies to
//! the colour write-back of D2, which needs a properties dialog with a
//! candidate. These two registrations connect them.
//!
//! Headless tests cannot do more. An `ESourceConfigBackend` is an `EExtension`
//! of an `ESourceConfig`, which is a `GtkBox`. Thus no instance can exist on a
//! machine without a display. The `insert_widgets` of [`crate::backend`] has
//! the same limit. For this reason, the one real decision here,
//! [`offers_creation_for`], is a plain function over a value, and the vfunc
//! around it is short. A test on this VM can thus reach the part that can be
//! wrong. The operator must click to confirm the remaining part.

use std::ffi::CStr;
use std::ptr;

use eds_sys::EExtensionClass;
use evo_sys::{
    E_CAL_CLIENT_SOURCE_TYPE_EVENTS, ECalClientSourceType, ECalSourceConfig, ESourceConfigBackend,
    ESourceConfigBackendClass, e_book_source_config_get_type, e_cal_source_config_get_source_type,
    e_cal_source_config_get_type, e_source_config_backend_get_config,
    e_source_config_backend_get_type, e_source_config_get_type,
};
use glib_sys::{GFALSE, GTRUE, GType, gboolean};
use jmap_backend_core::subclass::{ObjectSubclass, registered_type};
use jmap_backend_core::trampoline::guard;

/// The `BackendName` that both dialogs compare a candidate with.
///
/// It has the same spelling as [`crate::mail::MAIL_BACKEND_NAME`], but it is a
/// different thing. That one is a Camel provider's protocol. This one is the
/// value of `[Address Book] BackendName` / `[Calendar] BackendName` /
/// `[Collection] BackendName` in the registry. A test, not this comment, keeps
/// it equal to `jmap_collection_sync::child_source::BACKEND_NAME`, which each
/// discovered child contains.
///
/// The class field needs a `'static` pointer: Evolution keeps the class for the
/// life of the process and never frees it.
pub const BACKEND_NAME: &CStr = jmap_backend_core::BACKEND_NAME;

/// The instance struct of both subclasses: Evolution's, with no extension.
///
/// This module keeps no data per instance. Each decision of these backends
/// depends only on the scratch source that it gets. This is the Rust form of
/// `typedef ESourceConfigBackend EBookConfigLocal;` in Evolution's own modules.
#[repr(C)]
pub struct ConfigBackend {
    /// Evolution's. This code never reads it. It only gives it back as the
    /// instance pointer that Evolution supplied.
    parent: ESourceConfigBackend,
}

/// The class struct, also with no extension. GObject needs a size to allocate.
/// The important fields (the two names and the four vfunc slots) are the parent's.
#[repr(C)]
pub struct ConfigBackendClass {
    pub parent_class: ESourceConfigBackendClass,
}

/// `ESourceConfigBackend`, the parent of both types. This first registers the
/// type that its class initialiser requests.
///
/// Evolution's `e_source_config_backend_class_init` sets `extensible_type =
/// E_TYPE_SOURCE_CONFIG`. That is an accessor called from a class initialiser,
/// which is the deadlock that [`registered_type`] describes, in code that this
/// project cannot change. GLib initialises a parent's class under the same lock,
/// immediately before the child's class. Thus the only location to stop the
/// cycle is before both. This function registers `ESourceConfig` (and its parent
/// `GtkBox`) on a thread that holds no lock, because
/// [`ObjectSubclass::parent_type`] is resolved before registration starts. All
/// other references to the parent's class use this function for the same reason.
pub fn config_backend_type() -> GType {
    // SAFETY: no arguments, and the type system initialises itself. No class
    // initialiser calls this.
    unsafe {
        e_source_config_get_type();
        e_source_config_backend_get_type()
    }
}

/// The address book half. It is a type-level name, not a value.
///
/// No code makes an instance of it, because GObject allocates a
/// [`ConfigBackend`]. The marker only supplies the `NAME`, the parent and the
/// `class_init` that make the two registrations different.
pub enum JmapBookConfig {}

/// The calendar half. [`JmapBookConfig`] tells why it has no values.
pub enum JmapCalConfig {}

// SAFETY: both structs are #[repr(C)]. They start with the ESourceConfigBackend
// instance struct and class struct. `evo-sys/tests/source_config.rs` pins their
// layout against `g_type_query`. ESourceConfigBackend derives from GObject
// (through EExtension).
unsafe impl ObjectSubclass for JmapBookConfig {
    const NAME: &'static CStr = c"EBookConfigJmap";
    type Instance = ConfigBackend;
    type Class = ConfigBackendClass;

    fn parent_type() -> GType {
        config_backend_type()
    }

    fn class_init_types() -> Vec<GType> {
        // SAFETY: as `parent_type`.
        vec![unsafe { e_book_source_config_get_type() }]
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: the caller's contract, given directly to the shared body.
        // This looks up the dialog class and does not call its accessor. From
        // here, the accessor causes the deadlock that `registered_type`
        // describes. `class_init_types` has already registered the class.
        unsafe { init_class(class, registered_type(c"EBookSourceConfig")) };
    }
}

// SAFETY: as `JmapBookConfig`.
unsafe impl ObjectSubclass for JmapCalConfig {
    const NAME: &'static CStr = c"ECalConfigJmap";
    type Instance = ConfigBackend;
    type Class = ConfigBackendClass;

    fn parent_type() -> GType {
        config_backend_type()
    }

    fn class_init_types() -> Vec<GType> {
        // SAFETY: as `parent_type`.
        vec![unsafe { e_cal_source_config_get_type() }]
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: as `JmapBookConfig::class_init`.
        unsafe { init_class(class, registered_type(c"ECalSourceConfig")) };
        // The only overridden slot, and only this subclass overrides it.
        // `offers_creation_for` tells why the calendar needs it and the address
        // book does not.
        //
        // SAFETY: as above. The slot is in the parent's half of the class
        // struct, into which `init_class` wrote the names.
        unsafe { (*class).parent_class.allow_creation = Some(cal_allow_creation) };
    }
}

/// The shared steps of both `class_init`s, without the dialog class name.
///
/// # Safety
///
/// `class` must be a newly allocated class struct of one of the two types
/// above, with the parent class already copied into its first bytes. GObject
/// gives this to a `class_init`. `extensible` must be a registered `GType`, or 0
/// when the lookup failed. An `extensible_type` of 0 matches no dialog in
/// `e_extensible_load_extensions`, which is the safe failure. If inherited, it
/// matches both.
unsafe fn init_class(class: *mut ConfigBackendClass, extensible: GType) {
    // SAFETY: `class` starts with the parent's class struct, which contains
    // both name fields and all four vfunc slots.
    let backend = unsafe { &mut (*class).parent_class };

    backend.backend_name = BACKEND_NAME.as_ptr();
    // Already NULL from a parent class that never sets it. It is written
    // because it is a decision, not an omission. NULL sends this backend
    // through the *collection* loop of `init_candidates`, where the JMAP account
    // is the parent of the scratch source. A fixed uid sends it through the other
    // loop. This project registers no stub source, so that loop gives a
    // `g_warning` about an invalid parent_uid and a `continue`. The result is
    // the original symptom, with a line in the journal.
    backend.parent_uid = ptr::null();

    // SAFETY: ESourceConfigBackendClass starts with EExtensionClass.
    // `evo-sys/tests/source_config.rs` asserts this against the running type system.
    let extension = unsafe { &mut *ptr::from_mut(backend).cast::<EExtensionClass>() };
    // This field selects the dialog that makes a candidate for this type. If
    // inherited, it is `E_TYPE_SOURCE_CONFIG`, which is both dialogs.
    extension.extensible_type = extensible;
}

/// Returns whether a JMAP account offers to create the list type that a given
/// `ECalSourceConfig` configures.
///
/// One `ECalSourceConfig` class serves *New Calendar*, *New Task List* and *New
/// Memo List*. Only this value makes them different. Thus a calendar config
/// backend that inherits `allow_creation` (which always returns TRUE) is in all
/// three. For this reason, Evolution's own `cal-config-google` and
/// `cal-config-gtasks` each override it.
///
/// TRUE for events only. This project registers no task-list or memo-list
/// backend factory. Thus a source committed from *New Task List* would contain a
/// `[Task List] BackendName=jmap` that nothing can open. But it does not get
/// that far: the `requested_of` of `jmap_backend_collection::create_resource`
/// returns `None` for a source with neither the `[Address Book]` nor the
/// `[Calendar]` extension. EDS documents this as "cannot be determined without
/// ambiguity", and the create fails.
#[must_use]
pub fn offers_creation_for(source_type: ECalClientSourceType) -> bool {
    source_type == E_CAL_CLIENT_SOURCE_TYPE_EVENTS
}

/// [`offers_creation_for`] over the source type that the dialog configures.
///
/// ## Failure
///
/// FALSE on a panic, and on a config that Evolution has not attached yet. This
/// is the safe direction, and the `g_return_val_if_fail` of
/// `e_source_config_backend_allow_creation` does the same. The entry is then
/// not in the Type list, which the user can see. The alternative is an entry
/// that commits a source that no factory can open.
unsafe extern "C" fn cal_allow_creation(backend: *mut ESourceConfigBackend) -> gboolean {
    guard("ECalConfigJmap::allow_creation", GFALSE, || {
        // SAFETY: a live backend of this class, because Evolution dispatches
        // this slot on one. The config is `(transfer none)`: it is the return
        // value of `e_extension_get_extensible`, which lives longer than this call.
        let config = unsafe { e_source_config_backend_get_config(backend) };
        if config.is_null() {
            return GFALSE;
        }

        // SAFETY: an `ECalConfigJmap` is loaded into an extensible of the
        // `extensible_type` that its class names, an `ECalSourceConfig`.
        // `e_extensible_load_extensions` selects on this. The cast is C's
        // `E_CAL_SOURCE_CONFIG()`. `evo-sys/tests/source_config.rs` asks the
        // running type system to confirm this relation between the classes.
        let source_type =
            unsafe { e_cal_source_config_get_source_type(config.cast::<ECalSourceConfig>()) };

        if offers_creation_for(source_type) {
            GTRUE
        } else {
            GFALSE
        }
    })
}
