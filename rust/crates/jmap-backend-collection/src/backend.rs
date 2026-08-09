// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `ECollectionBackend` subclass itself.
//!
//! Deliberately dull, like `jmap-backend-book`'s: the instance and class
//! structs, the vfunc slots, and a panic guard in front of each body — the
//! bodies themselves live a layer down, where they can be tested without a
//! GObject. What is different here is that the slots are not empty to begin
//! with. `ECollectionBackendClass` installs a working `dup_resource_id` and a
//! do-nothing `populate`, so an override that is written but not *installed*
//! does not produce a backend that fails; it produces one that quietly answers
//! something else. `tests/backend.rs` holds each slot against the parent's to
//! keep that from being invisible.

use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::ptr;

use eds_sys::{
    E_SOURCE_CREDENTIALS_REASON_REQUIRED, ECollectionBackend, ECollectionBackendClass, ESource,
    e_backend_get_source, e_backend_schedule_authenticate, e_backend_schedule_credentials_required,
    e_collection_backend_claim_all_resources, e_collection_backend_freeze_populate,
    e_collection_backend_get_type, e_collection_backend_ref_server,
    e_collection_backend_thaw_populate, e_source_registry_debug_print,
    e_source_registry_server_add_source,
};
use glib_sys::{GFALSE, GType, g_list_free, gchar};
use gobject_sys::{g_object_unref, g_type_class_peek};
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::marshal::dup_string;
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_backend_core::trampoline::{guard, log_critical};
use jmap_collection_sync::Parts;

use crate::collection_source::{parts_of, user_of};
use crate::populate::Populating;
use crate::resource_id::resource_id_of;

/// The JMAP collection backend.
#[repr(C)]
pub struct JmapCollectionBackend {
    /// GObject's; never read by this code, only handed back to EDS as the
    /// instance pointer it gave us.
    parent: ECollectionBackend,
}

/// The class struct. Nothing of ours lives in it; it exists because GObject
/// needs a size to allocate and a place to put the vfunc slots.
#[repr(C)]
pub struct JmapCollectionBackendClass {
    pub parent_class: ECollectionBackendClass,
}

impl JmapCollectionBackend {
    /// An instance outside the GObject type system: zeroed parent bytes, which
    /// is what `instance_init` leaves behind minus the GObject.
    ///
    /// As in `jmap-backend-book`, this exists because a real instance needs an
    /// `ESourceRegistryServer` and so a running `evolution-source-registry` on
    /// the session bus, which neither the test VM nor CI has. Nothing may be
    /// touched through the result: the parent bytes are a valid bit pattern but
    /// they are not a GObject, so passing one to any EDS function is undefined
    /// behaviour.
    ///
    /// So this is sound for exactly one of the two vfuncs below —
    /// `dup_resource_id`, which never reads the backend at all, because its
    /// answer is a function of the child source. `populate` is the other kind:
    /// its very first act is `e_collection_backend_freeze_populate` on the
    /// instance, so driving it from one of these would be undefined behaviour.
    /// That is why the populate *body* is [`crate::populate`]'s, behind a trait,
    /// and why the only thing `tests/backend.rs` can say about the slot is that
    /// it is installed and is not the one it replaced.
    pub fn detached() -> Box<Self> {
        // SAFETY: every field of the parent is a pointer or an integer, for
        // which all-zero is a valid value.
        Box::new(unsafe { MaybeUninit::zeroed().assume_init() })
    }
}

// SAFETY: both structs are #[repr(C)] and lead with the ECollectionBackend
// instance and class structs respectively, and ECollectionBackend derives from
// GObject (via EBackend).
unsafe impl ObjectSubclass for JmapCollectionBackend {
    const NAME: &'static CStr = c"ECollectionBackendJmap";
    type Instance = JmapCollectionBackend;
    type Class = JmapCollectionBackendClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the EDS type system initialises itself.
        unsafe { e_collection_backend_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points at a freshly allocated class struct of ours,
        // which leads with the parent's; the slot below is in that half.
        let vfuncs = unsafe { &mut (*class).parent_class };
        vfuncs.dup_resource_id = Some(dup_resource_id);
        vfuncs.populate = Some(populate);
    }
}

/// The parent's class struct, for the one vfunc that chains up.
///
/// `g_type_class_peek` rather than `_ref`, as in `jmap-backend-book`: an
/// initialised class is what having registered a subclass of it guarantees, and
/// taking a reference here would mean giving one back on a path that has no
/// natural place to do so.
fn parent_class() -> Option<&'static ECollectionBackendClass> {
    // SAFETY: peeking a type that has never been referenced returns NULL, which
    // is handled; otherwise the class is alive for as long as the type is, which
    // for an EDS type is the life of the process.
    unsafe {
        g_type_class_peek(<JmapCollectionBackend as ObjectSubclass>::parent_type())
            .cast::<ECollectionBackendClass>()
            .as_ref()
    }
}

/// What EDS calls a cached child source to learn which collection it stands
/// for, once per file, before `populate`.
///
/// The `backend` argument is not read: the answer is a function of the child
/// source alone, which is what makes it testable without an instance — and what
/// makes it safe to drive from a detached one.
///
/// A panic becomes NULL, and NULL here means EDS deletes the file. That is the
/// wrong outcome, but there is no other: the vfunc has no `GError` and no
/// sentinel that means "ask me again". The guard logs a critical, which is the
/// only trace such a bug can leave.
unsafe extern "C" fn dup_resource_id(
    backend: *mut ECollectionBackend,
    child_source: *mut ESource,
) -> *mut gchar {
    let _ = backend;
    guard("dup_resource_id", ptr::null_mut(), || {
        // SAFETY: EDS hands us one of its own sources, alive for the call.
        match unsafe { resource_id_of(child_source) } {
            // SAFETY: ownership of the duplicate passes to EDS, which puts it
            // in its hash table and frees it with `g_free`.
            Some(id) => unsafe { dup_string(&id) },
            None => ptr::null_mut(),
        }
    })
}

/// What EDS calls when it wants a collection's children — on an idle as soon as
/// the account is added, on every reconnect, and whenever the account changes.
///
/// The decisions are [`crate::populate::populate`]'s; what is here is the account
/// read that a populate is not handed, and the report it has nowhere else to go
/// with. `e_source_registry_debug_print` is EDS's own debug channel, off unless
/// `SOURCE_REGISTRY_DEBUG` is set, and the same one
/// `e_collection_backend_new_child` writes its pairing line to.
///
/// A panic becomes a logged critical and no populate, which is the honest
/// outcome: the vfunc returns `void`, so there is nothing to report through, and
/// the next populate of this account will find the same state and try again. The
/// freeze is given back either way — see [`crate::populate`].
unsafe extern "C" fn populate(backend: *mut ECollectionBackend) {
    guard("populate", (), || {
        // `e_backend_get_source` is `(transfer none)` and cannot be NULL for a
        // backend EDS constructed *from* a source. If it somehow is, the cached
        // children are still exported — that needs no account — and nothing is
        // asked of a server this code cannot find, which is what an account with
        // no parts switched on gets too.
        // SAFETY: EDS hands us one of its own backends, alive for the call, and
        // `ECollectionBackend` derives from `EBackend`.
        let source = unsafe { e_backend_get_source(backend.cast()) };
        let (parts, user) = if source.is_null() {
            log_critical("populate: the collection backend has no account source");
            (Parts::NONE, None)
        } else {
            // SAFETY: a valid `ESource` owned by the backend, only read from.
            unsafe { (parts_of(source), user_of(source)) }
        };

        let collection = Live(backend);
        // SAFETY: `Live`'s methods are the EDS calls `Populating` documents, made
        // on a backend that is valid for the length of the vfunc.
        let report = unsafe { crate::populate::populate(&collection, parts, user.as_deref()) };
        // `None` is another populate of this account already running; it will do
        // the work.
        let Some(report) = report else { return };

        if report.unidentified > 0 {
            // Unreachable through EDS — it only caches a source `dup_resource_id`
            // answered for — so reaching it means a source changed underneath.
            log_critical(&format!(
                "populate: {} cached children of this account could not be named and stay hidden",
                report.unidentified
            ));
        }

        debug_print(&format!(
            "populate: {} cached children exported, asked to authenticate: {:?}",
            report.children.len(),
            report.asked
        ));
    });
}

/// A live collection, as a populate uses it: the seven EDS calls behind
/// [`Populating`], one line each.
///
/// This is the part of `populate` that cannot be tested on a machine with no
/// `evolution-source-registry` — see [`crate::populate`] on why everything else
/// is a layer down.
struct Live(*mut ECollectionBackend);

// SAFETY: `claim_all_resources` hands back what
// `e_collection_backend_claim_all_resources` referenced for us, one reference per
// source, and the freeze and thaw are the two halves of EDS's own counter.
unsafe impl Populating for Live {
    fn freeze(&self) -> bool {
        // SAFETY: a valid backend for the length of the vfunc this runs inside.
        unsafe { e_collection_backend_freeze_populate(self.0) != GFALSE }
    }

    fn thaw(&self) {
        // SAFETY: as above.
        unsafe { e_collection_backend_thaw_populate(self.0) };
    }

    fn chain_up(&self) {
        match parent_class().and_then(|class| class.populate) {
            // SAFETY: the parent's own populate, called on an instance of a type
            // derived from it, which is what chaining up means in C.
            Some(populate) => unsafe { populate(self.0) },
            // EDS installs a placeholder, so this cannot happen; if it ever
            // does, the populate below is still worth doing.
            None => log_critical("populate: the parent class has no populate to chain up to"),
        }
    }

    fn claim_all_resources(&self) -> Vec<*mut ESource> {
        // SAFETY: as above.
        let list = unsafe { e_collection_backend_claim_all_resources(self.0) };

        let mut sources = Vec::new();
        let mut node = list;
        while !node.is_null() {
            // SAFETY: a live node of the list just returned; its `data` is one of
            // the sources the claim referenced for us.
            sources.push(unsafe { (*node).data }.cast::<ESource>());
            // SAFETY: as above.
            node = unsafe { (*node).next };
        }

        // `g_list_free` and not `_full`: the nodes are ours to free and the
        // references in them pass on to the caller, which is what
        // `Populating`'s contract says they do.
        // SAFETY: a list this call owns, whose nodes nothing else holds.
        unsafe { g_list_free(list) };

        sources
    }

    fn publish(&self, child: *mut ESource) {
        // Referenced per child rather than once around the loop: `Populating` has
        // no setup call to hold one across, and a populate exports a handful of
        // children at most. `e_collection_backend_ref_server` reads a weak
        // reference, so NULL means the registry server is gone — during shutdown,
        // say — and then there is nothing to export to.
        // SAFETY: a valid backend; the server comes back `(transfer full)`.
        let server = unsafe { e_collection_backend_ref_server(self.0) };
        if server.is_null() {
            log_critical("populate: the registry server is gone; a child stays unexported");
            return;
        }

        // SAFETY: a live registry server and a live child source; the call takes
        // a reference of its own if it keeps the source.
        unsafe { e_source_registry_server_add_source(server, child) };
        // SAFETY: the reference `ref_server` handed over, not used again.
        unsafe { g_object_unref(server.cast()) };
    }

    fn request_credentials(&self) {
        // No certificate and no error: this backend never hands EDS a
        // certificate to offer the user (see `crate::authenticate`), and there is
        // no failed operation behind this — it is the first thing the account is
        // asked for.
        // SAFETY: a valid backend, which derives from `EBackend`; every other
        // argument is optional and NULL, and the reason is one of the enum's own
        // values.
        unsafe {
            e_backend_schedule_credentials_required(
                self.0.cast(),
                E_SOURCE_CREDENTIALS_REASON_REQUIRED,
                ptr::null(),
                0,
                ptr::null(),
                ptr::null_mut(),
                c"populate".as_ptr(),
            )
        };
    }

    fn authenticate_anonymously(&self) {
        // SAFETY: as above; NULL credentials are what an anonymous authenticate
        // is, and what `jmap_backend_core::marshal::password` reads as absent.
        unsafe { e_backend_schedule_authenticate(self.0.cast(), ptr::null()) };
    }
}

/// One line on EDS's own debug channel, which is silent unless
/// `SOURCE_REGISTRY_DEBUG` is set.
fn debug_print(message: &str) {
    let message = cstring_lossy(message);
    // SAFETY: `e_source_registry_debug_print` is variadic and takes a printf
    // format; passing the text as an argument to `%s` keeps a `%` in it from
    // being read as a directive.
    unsafe { e_source_registry_debug_print(c"%s\n".as_ptr(), message.as_ptr()) };
}
