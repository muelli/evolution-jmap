// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The two `ESourceConfigBackend` subclasses that put JMAP in Evolution's *New
//! Address Book* and *New Calendar* dialogs.
//!
//! ## What was broken
//!
//! Neither dialog offered JMAP at all, and opening an existing JMAP calendar's
//! properties showed an empty `Type:`, an empty `Name:`, a colour picker
//! reading black over a calendar Evolution was drawing blue, and an OK button
//! that did nothing. The journal named one cause four times:
//!
//! ```text
//! source_config_init_for_editing_source: assertion 'backend != NULL' failed
//! source_config_get_active_candidate:    assertion 'index >= 0' failed
//! e_source_config_check_complete:        assertion 'candidate != NULL' failed
//! e_source_config_commit:                assertion 'candidate != NULL' failed
//! ```
//!
//! `ESourceConfig` builds one *candidate* — one entry in the Type list, one
//! page of widgets, one commit target — per registered `ESourceConfigBackend`,
//! and this project registered none. `module-jmap-configuration` had two types
//! in it, an `EMailConfigServiceBackend` for mail account setup and an
//! `EConfigLookupWorker` for discovery, and neither is one of these.
//!
//! ## What a subclass has to say, and where each answer comes from
//!
//! Almost all of it is class fields rather than code, which is why this module
//! is short and its test file is not. Evolution's own
//! `module-book-config-local` is the same shape: a `class_init` and nothing
//! else. Read against `evolution-3.52.4/src/modules/` and
//! `src/e-util/e-source-config.c`, not from memory.
//!
//! - **`extensible_type`** decides which dialog the candidate appears in.
//!   `ESourceConfigBackend`'s own `class_init` sets it to `E_TYPE_SOURCE_CONFIG`
//!   — the base class — and inheriting that would put both subclasses in *both*
//!   dialogs, each sometimes handed the other's scratch source. So the address
//!   book subclass names `E_TYPE_BOOK_SOURCE_CONFIG` and the calendar one
//!   `E_TYPE_CAL_SOURCE_CONFIG`, exactly as Evolution's own modules do.
//! - **`backend_name`** is what a candidate is matched against, and the two
//!   routes into `source_config_init_candidates` match it against two different
//!   strings. Creating: the `[Collection] BackendName` of every source
//!   `list_eligible_collections` returns is looked up in the dialog's backend
//!   table. Editing: the existing source's own `[Address Book]`/`[Calendar]
//!   BackendName` is. Both are `"jmap"` here — [`crate::account`] writes the
//!   first, `jmap_collection_sync::child_source` the second — so one value
//!   serves both, and [`BACKEND_NAME`] is pinned against the second in
//!   `tests/source_config.rs`.
//! - **`parent_uid`** is left NULL, which is what routes these backends through
//!   the *collection* half of `init_candidates` rather than the fixed-parent
//!   half. A JMAP address book hangs off the account that discovered it, not
//!   off one of Evolution's `local-stub` placeholders.
//! - **`allow_creation`** is overridden on the calendar side only, and
//!   [`offers_creation_for`] says why.
//! - **`insert_widgets`, `check_complete` and `commit_changes`** are
//!   deliberately Evolution's own. There is nothing to ask beyond the name and
//!   colour the dialog already shows — the account knows the server, so unlike
//!   CardDAV there is no URL to type. Completeness is `ESourceConfig`'s own
//!   `check_complete`, which already refuses an empty Name and an unselected
//!   Type. And there is nothing extra to commit: writing the scratch source
//!   into the registry is precisely what makes `e-server-side-source.c` call
//!   this account's `create_resource_sync`, so a `commit_changes` of our own
//!   would be a second writer racing the one that does the work.
//!
//! ## What this reaches, and what it cannot
//!
//! [`jmap_backend_collection::create_resource`] and `delete_resource` were
//! written for D1 and have been unreachable from the GUI ever since, because
//! nothing could put a scratch source under a JMAP account for the registry to
//! act on. So was D2's colour write-back, which needs a properties dialog that
//! has a candidate. Registering these two is what connects them.
//!
//! Nothing headless can go further. An `ESourceConfigBackend` is an
//! `EExtension` of an `ESourceConfig`, which is a `GtkBox`, so no instance can
//! exist on a machine without a display — the same wall
//! [`crate::backend`]'s `insert_widgets` is behind. That is why the one real
//! decision here, [`offers_creation_for`], is a plain function over a value and
//! the vfunc around it is three lines: the part where a wrong answer is
//! possible is the part a test on this VM can reach. The rest is the operator's
//! to confirm by clicking.

use std::ffi::CStr;
use std::ptr;

use eds_sys::EExtensionClass;
use evo_sys::{
    E_CAL_CLIENT_SOURCE_TYPE_EVENTS, ECalClientSourceType, ECalSourceConfig, ESourceConfigBackend,
    ESourceConfigBackendClass, e_book_source_config_get_type, e_cal_source_config_get_source_type,
    e_cal_source_config_get_type, e_source_config_backend_get_config,
    e_source_config_backend_get_type,
};
use glib_sys::{GFALSE, GTRUE, GType, gboolean};
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_backend_core::trampoline::guard;

/// The `BackendName` both dialogs match a candidate against.
///
/// The same spelling as [`crate::mail::MAIL_BACKEND_NAME`] and a different
/// thing: that one is a Camel provider's protocol, this one is the value of
/// `[Address Book] BackendName` / `[Calendar] BackendName` / `[Collection]
/// BackendName` in the registry. They coincide, and a test rather than this
/// comment is what keeps this one tied to the source of truth —
/// `jmap_collection_sync::child_source::BACKEND_NAME`, which is what every
/// discovered child actually carries.
///
/// A `'static` pointer is what the class field means: Evolution keeps the class
/// for the life of the process and never frees it.
pub const BACKEND_NAME: &CStr = c"jmap";

/// The instance struct both subclasses use — Evolution's, unextended.
///
/// There is nothing of ours to keep per instance: every decision either of
/// these backends makes is a function of the scratch source it is handed. This
/// is the Rust spelling of the `typedef ESourceConfigBackend EBookConfigLocal;`
/// that Evolution's own modules open with.
#[repr(C)]
pub struct ConfigBackend {
    /// Evolution's; never read by this code, only handed back as the instance
    /// pointer it gave us.
    parent: ESourceConfigBackend,
}

/// The class struct, likewise unextended: it exists because GObject needs a
/// size to allocate, and the fields that matter — the two names and the four
/// vfunc slots — are all the parent's.
#[repr(C)]
pub struct ConfigBackendClass {
    pub parent_class: ESourceConfigBackendClass,
}

/// The address book half, and a type-level name rather than a value: no
/// instance of this is ever built here or anywhere else, since GObject
/// allocates a [`ConfigBackend`] and the marker exists only to carry the
/// `NAME`, the parent and the `class_init` that distinguish one registration
/// from the other.
pub enum JmapBookConfig {}

/// The calendar half. See [`JmapBookConfig`] for why it is uninhabited.
pub enum JmapCalConfig {}

// SAFETY: both structs are #[repr(C)] and lead with the ESourceConfigBackend
// instance and class structs respectively, whose layout
// `evo-sys/tests/source_config.rs` pins against `g_type_query`, and
// ESourceConfigBackend derives from GObject (via EExtension).
unsafe impl ObjectSubclass for JmapBookConfig {
    const NAME: &'static CStr = c"EBookConfigJmap";
    type Instance = ConfigBackend;
    type Class = ConfigBackendClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type system initialises itself.
        unsafe { e_source_config_backend_get_type() }
    }

    fn class_init_types() -> Vec<GType> {
        // SAFETY: as `parent_type`.
        vec![unsafe { e_book_source_config_get_type() }]
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: the caller's contract, passed straight to the shared body;
        // the GType is Evolution's own and registered by `class_init_types`.
        unsafe { init_class(class, e_book_source_config_get_type()) };
    }
}

// SAFETY: as `JmapBookConfig`.
unsafe impl ObjectSubclass for JmapCalConfig {
    const NAME: &'static CStr = c"ECalConfigJmap";
    type Instance = ConfigBackend;
    type Class = ConfigBackendClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type system initialises itself.
        unsafe { e_source_config_backend_get_type() }
    }

    fn class_init_types() -> Vec<GType> {
        // SAFETY: as `parent_type`.
        vec![unsafe { e_cal_source_config_get_type() }]
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: as `JmapBookConfig::class_init`.
        unsafe { init_class(class, e_cal_source_config_get_type()) };
        // The one slot either subclass overrides, and only this one does;
        // `offers_creation_for` says why the calendar side needs it and the
        // address book side does not.
        //
        // SAFETY: as above — the slot is in the parent's half of the class
        // struct `init_class` just wrote the names into.
        unsafe { (*class).parent_class.allow_creation = Some(cal_allow_creation) };
    }
}

/// Everything both `class_init`s do, minus which dialog class they name.
///
/// # Safety
///
/// `class` must be a freshly allocated class struct of one of the two types
/// above, with the parent class already copied into its leading bytes — which
/// is what GObject hands a `class_init`. `extensible` must be a registered
/// `GType`.
unsafe fn init_class(class: *mut ConfigBackendClass, extensible: GType) {
    // SAFETY: `class` leads with the parent's class struct, where both name
    // fields and all four vfunc slots live.
    let backend = unsafe { &mut (*class).parent_class };

    backend.backend_name = BACKEND_NAME.as_ptr();
    // Already NULL, inherited from a parent class that never sets it — written
    // anyway because it is a decision and not an omission. NULL is what sends
    // this backend down `init_candidates`' *collection* loop, where the parent
    // of the scratch source is the JMAP account; a fixed uid would send it down
    // the other one and, since we have no stub source registered, earn a
    // `g_warning` about an invalid parent_uid and a `continue` — the original
    // symptom back, with a line in the journal.
    backend.parent_uid = ptr::null();

    // SAFETY: ESourceConfigBackendClass leads with EExtensionClass, which
    // `evo-sys/tests/source_config.rs` asserts against the running type system.
    let extension = unsafe { &mut *ptr::from_mut(backend).cast::<EExtensionClass>() };
    // The field that decides which dialog builds a candidate for this type at
    // all. Inherited it would be `E_TYPE_SOURCE_CONFIG`, i.e. both of them.
    extension.extensible_type = extensible;
}

/// Whether a JMAP account offers to create the kind of list a given
/// `ECalSourceConfig` is configuring.
///
/// One `ECalSourceConfig` class serves *New Calendar*, *New Task List* and *New
/// Memo List*; nothing distinguishes them but this value. So a calendar config
/// backend that inherits `allow_creation` — which answers TRUE unconditionally
/// — offers itself in all three. Evolution's own `cal-config-google` and
/// `cal-config-gtasks` each override it for exactly this reason.
///
/// TRUE for events only. This project registers no task-list or memo-list
/// backend factory, so a source committed from *New Task List* would carry a
/// `[Task List] BackendName=jmap` that nothing can open — and it would not get
/// that far anyway: `jmap_backend_collection::create_resource`'s `requested_of`
/// answers `None` for a source carrying neither the `[Address Book]` nor the
/// `[Calendar]` extension, which is EDS's documented "cannot be determined
/// without ambiguity" and fails the create.
#[must_use]
pub fn offers_creation_for(source_type: ECalClientSourceType) -> bool {
    source_type == E_CAL_CLIENT_SOURCE_TYPE_EVENTS
}

/// [`offers_creation_for`] over the source type the dialog is configuring.
///
/// ## Failure
///
/// FALSE, on a panic and on a config Evolution has not attached yet. That is
/// the conservative direction and it is the same one `e_source_config_backend_
/// allow_creation`'s own `g_return_val_if_fail` takes: the entry is absent from
/// the Type list, which is visible, rather than present and committing a source
/// no factory can open.
unsafe extern "C" fn cal_allow_creation(backend: *mut ESourceConfigBackend) -> gboolean {
    guard("ECalConfigJmap::allow_creation", GFALSE, || {
        // SAFETY: a live backend of this class, which is what Evolution
        // dispatches through this slot. The config comes back
        // `(transfer none)` — `e_extension_get_extensible`'s answer, which
        // outlives this call.
        let config = unsafe { e_source_config_backend_get_config(backend) };
        if config.is_null() {
            return GFALSE;
        }

        // SAFETY: the extensible an `ECalConfigJmap` is loaded into is the
        // `extensible_type` its class names, i.e. an `ECalSourceConfig` — that
        // is what `e_extensible_load_extensions` selects on. The cast is C's
        // `E_CAL_SOURCE_CONFIG()`, over classes `evo-sys/tests/source_config.rs`
        // asks the running type system to confirm are related this way.
        let source_type =
            unsafe { e_cal_source_config_get_source_type(config.cast::<ECalSourceConfig>()) };

        if offers_creation_for(source_type) {
            GTRUE
        } else {
            GFALSE
        }
    })
}
