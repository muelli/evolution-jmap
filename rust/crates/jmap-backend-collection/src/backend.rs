// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `ECollectionBackend` subclass itself.
//!
//! Deliberately dull, like `jmap-backend-book`'s: the instance and class
//! structs, the vfunc slots, and a panic guard in front of each body — the
//! bodies themselves live a layer down, where they can be tested without a
//! GObject. What is different here is that none of the three slots is empty to
//! begin with. `ECollectionBackendClass` installs a working `dup_resource_id`
//! and a do-nothing `populate`, and `EBackendClass` — two levels up, which is
//! where `authenticate_sync` lives — installs one that reports success without
//! contacting anything. So an override that is written but not *installed* does
//! not produce a backend that fails; it produces one that quietly answers
//! something else, and in `authenticate_sync`'s case one that EDS believes
//! logged in. `tests/backend.rs` holds each slot against the parent's to keep
//! that from being invisible.

use std::ffi::CStr;
use std::ptr;

use std::ffi::CString;

use eds_sys::{
    E_SOURCE_AUTHENTICATION_ERROR, E_SOURCE_CREDENTIALS_REASON_REQUIRED, EBackend,
    ECollectionBackend, ECollectionBackendClass, ENamedParameters, ESource,
    ESourceAuthenticationResult, ESourceRegistryServer, e_backend_get_source,
    e_backend_schedule_authenticate, e_backend_schedule_credentials_required,
    e_collection_backend_claim_all_resources, e_collection_backend_freeze_populate,
    e_collection_backend_get_cache_dir, e_collection_backend_get_type,
    e_collection_backend_is_new_source, e_collection_backend_list_calendar_sources,
    e_collection_backend_list_contacts_sources, e_collection_backend_new_child,
    e_collection_backend_ref_server, e_collection_backend_thaw_populate,
    e_server_side_source_set_remote_creatable, e_source_get_uid, e_source_registry_debug_print,
    e_source_registry_server_add_source,
};
use gio_sys::{GCancellable, GTlsCertificateFlags};
use glib_sys::{GError, GFALSE, GList, GTRUE, GType, g_list_free, gboolean, gchar};
use gobject_sys::g_type_class_peek;
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::{cstring_lossy, fail_bool, fail_invalid};
#[cfg(feature = "testing")]
use jmap_backend_core::instance::zeroed_box;
use jmap_backend_core::marshal::{dup_string, read_string};
use jmap_backend_core::owned::Owned;
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_backend_core::trampoline::{guard, guard_bool, guard_value, log_critical};
use jmap_collection_sync::Parts;

use crate::authenticate::{Login, authenticate_with, login_of};
use crate::child_added::follow_collection;
use crate::collection_source::{parts_of, user_of};
use crate::create_resource::{
    CreateError, adopt_created, create_on_server, kind_noun, requested_of, stored_password_of,
};
use crate::delete_resource::{DeleteError, delete_on_server, doomed_of, offer_deletion};
use crate::fan_out::{Collection, Populated, fan_out};
use crate::populate::Populating;
use crate::removal::remove_source;
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
    #[cfg(feature = "testing")]
    pub fn detached() -> Box<Self> {
        // SAFETY: every field of the parent is a pointer or an integer, for
        // which all-zero is a valid value.
        unsafe { zeroed_box() }
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
        vfuncs.child_added = Some(child_added);
        // The two slots whose EDS default is a *refusal* rather than a wrong
        // answer: `collection_backend_create_resource()` and its delete twin
        // return `G_IO_ERROR_NOT_SUPPORTED` and nothing else, so leaving either
        // uninstalled is an account that cannot create or delete address books,
        // which is visible. Which is why these two overrides, unlike
        // `child_added`'s, must not chain up — see `crate::create_resource` and
        // `crate::delete_resource`.
        vfuncs.create_resource_sync = Some(create_resource_sync);
        vfuncs.delete_resource_sync = Some(delete_resource_sync);
        // A grandparent's slot, not the collection backend's own: EDS's own
        // default for it accepts every account without contacting anything, so
        // the one thing worse than not writing here is writing to the wrong
        // offset. `tests/backend.rs` holds both.
        vfuncs.parent_class.authenticate_sync = Some(authenticate_sync);
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

/// What EDS calls for every source that appears under this collection — the
/// children a fan-out just wrote, the cached ones a populate exported, and the
/// mail sources this backend neither creates nor caches.
///
/// The decisions are [`crate::child_added::follow_collection`]'s; what is here
/// is the account source the vfunc is not handed, and the chain-up.
///
/// The chain-up goes **first**, which is the other way round from
/// `e_ews_backend_child_added`. The parent's implementation is what puts the
/// child in the backend's own table and binds its enabled flag to the account's
/// — so it is what makes `e_collection_backend_list_*_sources` know about it,
/// which is what the next fan-out's removal pass reads. A panic in the binding
/// below must not cost the child that; a panic in the chain-up, which is EDS's
/// own code, is not a thing this order can help with either way.
///
/// A missing account source is a logged critical and no binding, as in
/// `populate`: the child is already exported by then and works with whatever it
/// was written with; what it loses is only its following of an account this code
/// cannot find.
unsafe extern "C" fn child_added(backend: *mut ECollectionBackend, child_source: *mut ESource) {
    guard("child_added", (), || {
        match parent_class().and_then(|class| class.child_added) {
            // SAFETY: the parent's own child_added, called on an instance of a
            // type derived from it, which is what chaining up means in C.
            Some(child_added) => unsafe { child_added(backend, child_source) },
            // EDS installs its own, so this cannot happen; if it ever does, the
            // binding below is still worth making.
            None => log_critical("child_added: the parent class has no child_added to chain up to"),
        }

        // Every child of a collection passes through here exactly once —
        // fanned-out, drawn from the cache by a populate, or just published by a
        // create — which is what makes this the one place the flag is written,
        // and the same place EDS's own `collection_backend_child_added` writes
        // `removable = FALSE`. It is deliberately before the account read below:
        // whether a child may be deleted from the server is a property of the
        // child alone, so an account this code cannot find must not cost the
        // user the menu item.
        // SAFETY: a child source of a collection, which
        // `evolution-source-registry` holds only as `EServerSideSource`s, alive
        // for the length of the vfunc.
        unsafe { offer_deletion(child_source) };

        // `(transfer none)`, and NULL only for a backend EDS did not construct
        // from a source.
        // SAFETY: EDS hands us one of its own backends, alive for the call, and
        // `ECollectionBackend` derives from `EBackend`.
        let source = unsafe { e_backend_get_source(backend.cast()) };
        if source.is_null() {
            log_critical("child_added: the collection backend has no account source");
            return;
        }

        // SAFETY: the account source EDS owns and one of its child sources, both
        // alive for the length of the vfunc.
        unsafe { follow_collection(source, child_source) };
    });
}

/// What EDS calls once it has resolved the account's credentials — after a
/// `populate` asked it to, and again whenever Evolution retries a login.
///
/// The decisions are [`crate::authenticate::authenticate_with`]'s, and the
/// children are [`crate::fan_out::fan_out`]'s; what is here is the instance
/// those two are not given and the report the fan-out has nowhere else to go
/// with. Everything before the network — which accounts are contacted at all,
/// which failures become a second password prompt — is decided in
/// `authenticate_with`, where it is testable.
///
/// The two certificate out-parameters are deliberately left alone; see
/// [`crate::authenticate`] on why this backend never offers a certificate for
/// the user to trust.
///
/// A panic becomes `E_SOURCE_AUTHENTICATION_ERROR` with the error set. Not
/// `REJECTED`, which would make EDS throw away a password that is probably
/// correct and ask for it again on every retry; a bug in this code is not the
/// user's password being wrong.
unsafe extern "C" fn authenticate_sync(
    backend: *mut EBackend,
    credentials: *const ENamedParameters,
    out_certificate_pem: *mut *mut gchar,
    out_certificate_errors: *mut GTlsCertificateFlags,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> ESourceAuthenticationResult {
    let _ = (out_certificate_pem, out_certificate_errors);

    let authenticate = || {
        // `(transfer none)`, and NULL only for a backend EDS did not construct
        // from a source; `authenticate_with` answers that with an error of its
        // own rather than with a prompt.
        // SAFETY: EDS hands us one of its own backends, alive for the call.
        let source = unsafe { e_backend_get_source(backend) };
        // SAFETY: our instance is an `ECollectionBackend`, which is what EDS
        // dispatched this vfunc on.
        let collection = Live(backend.cast());

        // SAFETY: the arguments are the vfunc's own, which is exactly
        // `authenticate_with`'s contract, and `Live`'s methods are the EDS
        // calls `Collection` documents.
        unsafe {
            authenticate_with(source, credentials, cancellable, error, |login| {
                let report = fan_out(&collection, &login)?;
                report_fan_out(&report);
                Ok(())
            })
        }
    };

    // SAFETY: `error` is what an EDS vfunc receives — NULL or a pointer to a
    // NULL `GError`. `authenticate_with` sets it only on paths it then returns
    // from, so a panic can never unwind past an already-set error into the
    // guard's own, which would be one `GError` overwriting another.
    unsafe {
        guard_value(
            "authenticate_sync",
            error,
            E_SOURCE_AUTHENTICATION_ERROR,
            authenticate,
        )
    }
}

/// What EDS calls when the user asks Evolution for a new address book or
/// calendar *in this account* — the one request that creates something on the
/// server rather than mirroring what is there.
///
/// The decisions are [`crate::create_resource`]'s and
/// [`jmap_collection_sync::create`]'s; what is here is the four things only a
/// live instance can answer — the account source, its uid, the collection's cache
/// directory and the registry server — and the order they are needed in.
///
/// **No chain-up.** The parent's implementation is the
/// `G_IO_ERROR_NOT_SUPPORTED` this override replaces; calling it on a path this
/// code handled would report a create that worked as a failure. See
/// [`crate::create_resource`] on where that is written down in EDS's source.
///
/// The publish is last and unconditional once the source is written: EDS's own
/// documentation of the vfunc makes adding an `ESource` to the server part of the
/// contract, not an optimisation, and an unexported child would make a create
/// that succeeded look as though nothing had happened until the next populate.
/// It is not gated on `e_collection_backend_is_new_source` the way
/// [`crate::fan_out`]'s is — that question is about a child drawn from the
/// backend's cache, and this source came from EDS's `remote_create` handler,
/// which never consults it.
///
/// A panic becomes `FALSE` with the error set, which is
/// [`guard_bool`](jmap_backend_core::trampoline::guard_bool)'s contract and the
/// right one here: the collection may or may not exist on the server afterwards,
/// and the next populate is what reconciles that — exactly as it does for a
/// create whose source write failed.
unsafe extern "C" fn create_resource_sync(
    backend: *mut ECollectionBackend,
    scratch_source: *mut ESource,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    let create = || {
        // The kind and the name, off the scratch source EDS built from the
        // keyfile Evolution sent. A source naming no kind is EDS's documented
        // "cannot be determined without ambiguity".
        // SAFETY: EDS hands us the scratch source it created, alive for the call.
        let Some(requested) = (unsafe { requested_of(scratch_source) }) else {
            // SAFETY: `error` is what an EDS vfunc receives, and nothing has
            // written to it yet.
            return unsafe { fail_bool(error, &CreateError::UnknownKind, CreateError::to_gerror) };
        };

        // `(transfer none)`, and NULL only for a backend EDS did not construct
        // from a source — which is also the case in which there is no account to
        // read a server off and no uid to parent the child to, so it is an error
        // here rather than the logged critical `populate` answers it with.
        // SAFETY: EDS hands us one of its own backends, alive for the call, and
        // `ECollectionBackend` derives from `EBackend`.
        let account = unsafe { e_backend_get_source(backend.cast()) };
        if account.is_null() {
            // SAFETY: as above.
            return unsafe {
                fail_invalid(
                    error,
                    "the JMAP collection backend has no account to create a collection in",
                )
            };
        }

        // SAFETY: a live backend, a valid account source and a cancellable that
        // is NULL or EDS's own — `login_for`'s contract exactly.
        let login =
            match unsafe { login_for(backend, account, cancellable, "create_resource_sync") } {
                Ok(login) => login,
                // SAFETY: `error` is untouched so far.
                Err(failure) => {
                    return unsafe {
                        fail_bool(error, &CreateError::from(failure), CreateError::to_gerror)
                    };
                }
            };

        // Held for the length of the create and no longer, as in
        // `authenticate_sync`.
        // SAFETY: `cancellable` is NULL or a valid `GCancellable` EDS keeps alive
        // for the duration of the vfunc.
        let _cancel = unsafe { observe(cancellable) };

        let child =
            match create_on_server(&login.server.target, login.credentials.clone(), &requested) {
                Ok(child) => child,
                // SAFETY: as above.
                Err(failure) => {
                    return unsafe { fail_bool(error, &failure, CreateError::to_gerror) };
                }
            };

        // SAFETY: a valid account source; both getters answer NULL or a string
        // the object owns.
        let (account_uid, cache_dir) = unsafe {
            (
                read_string(e_source_get_uid(account)),
                read_string(e_collection_backend_get_cache_dir(backend)),
            )
        };
        let Some(account_uid) = account_uid else {
            // A source with no uid cannot be a parent, and every source the
            // registry loaded has one — its file name.
            // SAFETY: as above.
            return unsafe {
                fail_invalid(
                    error,
                    "the JMAP collection backend's account has no uid to parent a new \
                     collection to",
                )
            };
        };

        // SAFETY: the scratch source is the `EServerSideSource` EDS's
        // `remote_create` handler built, alive for the call.
        if let Err(setting) = unsafe {
            adopt_created(
                scratch_source,
                &child,
                &login.server.connection,
                &account_uid,
                cache_dir.as_deref(),
            )
        } {
            // The collection is on the server and this child is not written, so
            // nothing is exported — see `adopt_created` on why that is the honest
            // answer and what the next populate does about it.
            // SAFETY: as above.
            return unsafe {
                fail_bool(
                    error,
                    &CreateError::Unwritable(child.kind, setting),
                    CreateError::to_gerror,
                )
            };
        }

        // The second of EDS's two documented obligations: a created resource
        // that is not added to the server is one Evolution does not see until
        // the next populate, so the create would look as if nothing had
        // happened.
        Collection::publish(&Live(backend), scratch_source);
        debug_print(&format!(
            "create_resource_sync: created {} {}",
            kind_noun(child.kind),
            child.resource_id
        ));
        GTRUE
    };

    // SAFETY: `error` is what an EDS vfunc receives — NULL or a pointer to a
    // NULL `GError`. Every path above that sets it then returns, so a panic can
    // never unwind past an already-set error into the guard's own.
    unsafe { guard_bool("create_resource_sync", error, create) }
}

/// What EDS calls when the user chooses "Delete" on an address book or a
/// calendar of this account — the one request that destroys something on the
/// server.
///
/// The decisions are [`crate::delete_resource`]'s and
/// [`jmap_collection_sync::delete`]'s; what is here is the two things only a
/// live instance can answer — the account source and, through it, the account's
/// credentials — and the order the three steps go in.
///
/// **No chain-up**, for the reason `create_resource_sync` gives: the parent's
/// implementation is the `G_IO_ERROR_NOT_SUPPORTED` this override replaces.
///
/// **The destroy comes before the removal**, and that order is the error
/// handling — see [`crate::delete_resource`] on which of the two half-done
/// states is recoverable and which loses the child's uid and offline cache for
/// nothing.
///
/// A panic becomes `FALSE` with the error set. The collection may or may not
/// still exist on the server afterwards, and the next populate is what
/// reconciles that — the same answer a failed create gets.
unsafe extern "C" fn delete_resource_sync(
    backend: *mut ECollectionBackend,
    child_source: *mut ESource,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    let delete = || {
        // Which collection this source stands for, read the same way
        // `dup_resource_id` reads it. A source that answers nothing is not one
        // of ours, and nothing of ours may be destroyed for it.
        // SAFETY: EDS hands us one of its own sources, alive for the call.
        let Some(doomed) = (unsafe { doomed_of(child_source) }) else {
            // SAFETY: `error` is what an EDS vfunc receives, and nothing has
            // written to it yet.
            return unsafe { fail_bool(error, &DeleteError::NotOurs, DeleteError::to_gerror) };
        };

        // `(transfer none)`, and NULL only for a backend EDS did not construct
        // from a source — which is also the case in which there is no account to
        // read a server off, so it is an error here rather than the logged
        // critical `populate` answers it with.
        // SAFETY: EDS hands us one of its own backends, alive for the call, and
        // `ECollectionBackend` derives from `EBackend`.
        let account = unsafe { e_backend_get_source(backend.cast()) };
        if account.is_null() {
            // SAFETY: as above.
            return unsafe {
                fail_invalid(
                    error,
                    "the JMAP collection backend has no account to delete a collection from",
                )
            };
        }

        // SAFETY: a live backend, a valid account source and a cancellable that
        // is NULL or EDS's own — `login_for`'s contract exactly.
        let login =
            match unsafe { login_for(backend, account, cancellable, "delete_resource_sync") } {
                Ok(login) => login,
                // SAFETY: `error` is untouched so far.
                Err(failure) => {
                    return unsafe {
                        fail_bool(error, &DeleteError::from(failure), DeleteError::to_gerror)
                    };
                }
            };

        // Held for the length of the delete and no longer, as in
        // `create_resource_sync`.
        // SAFETY: `cancellable` is NULL or a valid `GCancellable` EDS keeps alive
        // for the duration of the vfunc.
        let _cancel = unsafe { observe(cancellable) };

        if let Err(failure) =
            delete_on_server(&login.server.target, login.credentials.clone(), &doomed)
        {
            // SAFETY: as above.
            return unsafe { fail_bool(error, &failure, DeleteError::to_gerror) };
        }

        // EDS's documented obligation, and only now that the collection is
        // really gone: "the implementor must also remove @source from the
        // @backend's #ECollectionBackend:server". `e_source_remove_sync` on a
        // server-side source is that removal — it unexports the object and
        // deletes the key file — which is why this is `crate::removal`'s call
        // rather than a second one written here.
        // SAFETY: a child source of this collection, alive for the call, and a
        // cancellable that is NULL or EDS's own.
        if let Err(reason) = unsafe { remove_source(child_source, cancellable) } {
            // SAFETY: as above.
            return unsafe {
                fail_bool(error, &DeleteError::Stale(reason), DeleteError::to_gerror)
            };
        }

        debug_print(&format!(
            "delete_resource_sync: deleted {} {}",
            kind_noun(doomed.kind),
            doomed.collection_id
        ));
        GTRUE
    };

    // SAFETY: as `create_resource_sync`'s — every path above that sets the error
    // then returns, so a panic can never unwind past an already-set one.
    unsafe { guard_bool("delete_resource_sync", error, delete) }
}

/// The account's [`Login`], for the two vfuncs EDS hands no `ENamedParameters`.
///
/// `authenticate_sync` is given the credentials EDS resolved; `create_resource_sync`
/// and `delete_resource_sync` are not, so they have to ask for them — see
/// [`crate::create_resource`] on why the store is asked at the moment the
/// password is needed rather than a secret being held for the life of the
/// account. An OAuth 2.0 account ignores the password entirely and gets a fresh
/// token inside [`login_of`].
///
/// `context` names the calling vfunc, for the critical channel.
///
/// # Safety
///
/// `backend` must be a valid `ECollectionBackend`, `account` the valid `ESource`
/// EDS constructed it from, and `cancellable` NULL or a valid `GCancellable` —
/// which is what both vfuncs receive.
unsafe fn login_for(
    backend: *mut ECollectionBackend,
    account: *mut ESource,
    cancellable: *mut GCancellable,
    context: &str,
) -> Result<Login, crate::authenticate::LoginError> {
    // SAFETY: a valid backend by this function's contract; the server comes back
    // `(transfer full)` and is dropped, releasing the reference, before it goes
    // out of scope.
    let password = unsafe {
        let server =
            Owned::<ESourceRegistryServer>::from_raw(e_collection_backend_ref_server(backend));
        let server_ptr = server.as_ref().map_or(ptr::null_mut(), Owned::as_ptr);
        stored_password_of(server_ptr, account, cancellable, context)
    };

    // SAFETY: a valid account source and a cancellable that satisfies
    // `login_of`'s contract by this function's own.
    unsafe { login_of(account, parts_of(account), password.as_deref(), cancellable) }
}

/// What one fan-out did, on the two channels a vfunc that has already answered
/// still has.
///
/// The three failure lists are criticals because each of them is a child the
/// user asked for and will not see, and none of them is anything the user can
/// fix: `uncreated` is EDS refusing to claim a resource, `abandoned` is a
/// setting this crate was never taught to write, and `not_removed` is a
/// deletion EDS refused. The successes go to EDS's own debug channel, silent
/// unless `SOURCE_REGISTRY_DEBUG` is set.
///
/// None of it reaches the `GError`: a login that worked is `ACCEPTED` even if
/// one address book of it could not be written, and turning a per-child
/// failure into a failed authentication would take the whole account offline
/// over one collection.
fn report_fan_out(report: &Populated) {
    for resource_id in &report.uncreated {
        log_critical(&format!(
            "authenticate_sync: EDS would not create a child source for {resource_id}"
        ));
    }
    for abandoned in &report.abandoned {
        log_critical(&format!(
            "authenticate_sync: {} stays unexported: {}",
            abandoned.resource_id, abandoned.setting
        ));
    }
    for not_removed in &report.not_removed {
        log_critical(&format!(
            "authenticate_sync: {} should have been removed and was not: {}",
            not_removed.resource_id, not_removed.message
        ));
    }

    debug_print(&format!(
        "authenticate_sync: {} children written",
        report.children.len()
    ));
}

/// A live collection, as a populate and a fan-out use it: the EDS calls behind
/// [`Populating`] and [`Collection`], one line each.
///
/// This is the part of both that cannot be tested on a machine with no
/// `evolution-source-registry` — see [`crate::populate`] and [`crate::fan_out`]
/// on why everything else is a layer down.
struct Live(*mut ECollectionBackend);

impl Live {
    /// `e_source_registry_server_add_source (server, child)`, which both traits
    /// call `publish` and which is one call on the server rather than on the
    /// backend.
    ///
    /// The server is referenced per child rather than once around a loop:
    /// neither trait has a setup call to hold one across, and a fan-out exports
    /// a handful of children at most. `e_collection_backend_ref_server` reads a
    /// weak reference, so NULL means the registry server is gone — during
    /// shutdown, say — and then there is nothing to export to.
    fn export(&self, child: *mut ESource, context: &str) {
        // SAFETY: a valid backend; the server comes back `(transfer full)`.
        let server = unsafe {
            Owned::<ESourceRegistryServer>::from_raw(e_collection_backend_ref_server(self.0))
        };
        let Some(server) = server else {
            log_critical(&format!(
                "{context}: the registry server is gone; a child stays unexported"
            ));
            return;
        };

        // SAFETY: a live registry server and a live child source; the call takes
        // a reference of its own if it keeps the source.
        unsafe { e_source_registry_server_add_source(server.as_ptr(), child) };
    }
}

/// The `ESource`s of a `(transfer full)` `GList`, with the list freed and every
/// reference in it passed on to the caller.
///
/// `g_list_free` and not `_full`: that is what both
/// `e_collection_backend_claim_all_resources()` and
/// `e_collection_backend_list_*_sources()` document, and what both
/// [`Populating`] and [`Collection`] promise their callers.
///
/// # Safety
///
/// `list` must be NULL or a `GList` this call may free, whose `data` are
/// `ESource *` carrying one reference each.
unsafe fn drain(list: *mut GList) -> Vec<*mut ESource> {
    let mut sources = Vec::new();
    let mut node = list;
    while !node.is_null() {
        // SAFETY: a live node of the list, whose `data` is one of the sources
        // referenced for us.
        sources.push(unsafe { (*node).data }.cast::<ESource>());
        // SAFETY: as above.
        node = unsafe { (*node).next };
    }

    // SAFETY: a list this call owns, whose nodes nothing else holds.
    unsafe { g_list_free(list) };

    sources
}

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
        // SAFETY: as above; the claim answers `(transfer full)`, one reference
        // per source, which is what `drain` passes on.
        unsafe { drain(e_collection_backend_claim_all_resources(self.0)) }
    }

    fn publish(&self, child: *mut ESource) {
        self.export(child, "populate");
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

    fn offer_creation(&self, offer: bool) {
        // The account source, not a child: this is the flag Evolution reads to
        // decide whether "New Address Book" may target this account.
        //
        // In `populate` rather than in a `constructed` override, which is what
        // evolution-ews uses: this crate has no `constructed`, and adding one
        // would mean writing a slot in `GObjectClass` — a third class struct,
        // further up than `authenticate_sync`'s `EBackendClass` — for a
        // one-line effect. `populate` is the first thing EDS calls on a
        // collection backend and it runs again on every reconnect and every
        // account change, which is exactly the cadence a flag derived from the
        // account's own settings wants.
        // SAFETY: a valid backend for the length of the vfunc this runs inside;
        // `e_backend_get_source` is `(transfer none)`. The account source of a
        // backend the registry constructed is an `EServerSideSource` — a source
        // that somehow is not gets EDS's own `g_return_if_fail` critical rather
        // than undefined behaviour — and NULL is answered by doing nothing,
        // since a backend with no account has no flag to write.
        unsafe {
            let source = e_backend_get_source(self.0.cast());
            if source.is_null() {
                return;
            }
            e_server_side_source_set_remote_creatable(
                source.cast(),
                if offer { GTRUE } else { GFALSE },
            );
        }
    }
}

// SAFETY: `new_child` and `existing_children` both hand back what EDS
// referenced for us, one reference per source — `(transfer full)` in both
// cases, which is what `Collection` asks for.
unsafe impl Collection for Live {
    fn new_child(&self, resource_id: &str) -> *mut ESource {
        // `CString::new` and not `cstring_lossy`: truncating at an interior NUL
        // would not fail, it would silently ask EDS for a *different* resource
        // and pair this collection's child with it. A resource id EDS cannot be
        // asked about is one no child exists for, which is what NULL means here
        // and what `adopt` reports as `Uncreated`. Reachable only from a server
        // that put a NUL in an id, since every other resource id in this crate
        // was read back out of a C string.
        let Ok(resource_id) = CString::new(resource_id) else {
            return ptr::null_mut();
        };
        // SAFETY: a valid backend for the length of the vfunc this runs inside,
        // and a NUL-terminated string the call only reads from.
        unsafe { e_collection_backend_new_child(self.0, resource_id.as_ptr()) }
    }

    fn is_new_child(&self, child: *mut ESource) -> bool {
        // SAFETY: a valid backend and one of the child sources it just handed
        // back, both alive for this call.
        unsafe { e_collection_backend_is_new_source(self.0, child) != GFALSE }
    }

    fn publish(&self, child: *mut ESource) {
        self.export(child, "authenticate_sync");
    }

    fn existing_children(&self) -> Vec<*mut ESource> {
        // Address books and calendars, in that order and never mail: this
        // backend creates no mail children, so it has no opinion about them and
        // must not remove them. Two calls rather than one because EDS has no
        // "every child" accessor that is not also every mail source.
        // SAFETY: a valid backend; each list is `(transfer full)`, one
        // reference per source, which is what `drain` passes on.
        let mut children = unsafe { drain(e_collection_backend_list_contacts_sources(self.0)) };
        // SAFETY: as above.
        children.extend(unsafe { drain(e_collection_backend_list_calendar_sources(self.0)) });
        children
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
