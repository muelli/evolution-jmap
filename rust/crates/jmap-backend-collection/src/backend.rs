// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `ECollectionBackend` subclass itself.
//!
//! Deliberately simple, as in `jmap-backend-book`: the instance and class
//! structs, the vfunc slots, and a panic guard in front of each body. The
//! bodies are one layer down, where tests can run them without a GObject.
//!
//! No slot here is empty in the parent. `ECollectionBackendClass` installs a
//! working `dup_resource_id` and an empty `populate`. `EBackendClass`, two
//! levels up, installs an `authenticate_sync` that reports success and
//! contacts nothing. Thus an override that is not *installed* does not fail:
//! the backend silently gives a different answer. For `authenticate_sync`,
//! EDS then thinks that the login succeeded. `tests/backend.rs` compares each
//! slot with the slot of the parent, so that this error is visible.

use std::ffi::CStr;
use std::ptr;
use std::time::SystemTime;

use std::ffi::CString;

use eds_sys::{
    E_SOURCE_AUTHENTICATION_ERROR, E_SOURCE_CREDENTIALS_REASON_REQUIRED,
    E_SOURCE_EXTENSION_COLLECTION, EBackend, ECollectionBackend, ECollectionBackendClass,
    ENamedParameters, ESource, ESourceAuthenticationResult, ESourceCollection,
    ESourceRegistryServer, e_backend_get_source, e_backend_schedule_authenticate,
    e_backend_schedule_credentials_required, e_collection_backend_authenticate_children,
    e_collection_backend_claim_all_resources, e_collection_backend_freeze_populate,
    e_collection_backend_get_cache_dir, e_collection_backend_get_type,
    e_collection_backend_is_new_source, e_collection_backend_list_calendar_sources,
    e_collection_backend_list_contacts_sources, e_collection_backend_new_child,
    e_collection_backend_ref_server, e_collection_backend_thaw_populate,
    e_server_side_source_get_exported, e_server_side_source_set_remote_creatable,
    e_server_side_source_set_remote_deletable, e_source_collection_set_allow_sources_rename,
    e_source_get_enabled, e_source_get_extension, e_source_get_uid, e_source_registry_debug_print,
    e_source_registry_server_add_source, e_source_set_enabled,
};
use gio_sys::{GCancellable, GTlsCertificateFlags};
use glib_sys::{
    GError, GFALSE, GList, GTRUE, GType, g_list_free, gboolean, gchar, gpointer, guint16,
};
use gobject_sys::{G_CONNECT_DEFAULT, g_object_ref, g_object_unref, g_signal_connect_object};
use jmap_backend_core::callback::gcallback;
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::{cstring_lossy, fail_bool, fail_invalid};
#[cfg(feature = "testing")]
use jmap_backend_core::instance::zeroed_box;
use jmap_backend_core::marshal::{dup_string, read_string};
use jmap_backend_core::oauth2::source_uses_oauth2;
use jmap_backend_core::owned::Owned;
use jmap_backend_core::source::destination_address;
use jmap_backend_core::subclass::{self, ObjectSubclass};
use jmap_backend_core::trampoline::{
    guard, guard_bool, guard_value, log_critical, log_critical_for_account,
    log_critical_for_resource,
};
use jmap_collection_sync::Parts;

use crate::authenticate::{Login, authenticate_with, login_of};
use crate::child_added::follow_collection;
use crate::collection_source::{parts_of, user_of};
use crate::create_resource::{
    CreateError, adopt_created, create_on_server, kind_noun, requested_of, stored_password_of,
};
use crate::deferred_delete::{PendingChild, delete_later, watch};
use crate::delete_resource::{DeleteError, delete_on_server, doomed_of, offer_deletion};
use crate::fan_out::{Collection, Populated, fan_out};
use crate::populate::Populating;
use crate::removal::remove_source;
use crate::resource_id::resource_id_of;
use crate::source_changed::{AccountWatch, login_fingerprint};

/// The JMAP collection backend.
#[repr(C)]
pub struct JmapCollectionBackend {
    /// The parent instance. This code does not read it. It only gives it back
    /// to EDS as the instance pointer.
    parent: ECollectionBackend,
    /// The `"changed"` connection of the account, and if its settings are
    /// proven to work. Refer to [`crate::source_changed`].
    watch: AccountWatch,
}

/// The class struct. It holds no data of this crate. GObject needs it for the
/// allocation size and for the vfunc slots.
#[repr(C)]
pub struct JmapCollectionBackendClass {
    pub parent_class: ECollectionBackendClass,
}

impl JmapCollectionBackend {
    /// An instance outside the GObject type system: zeroed parent bytes, as
    /// `instance_init` leaves them, but with no GObject.
    ///
    /// As in `jmap-backend-book`, a real instance needs an
    /// `ESourceRegistryServer`, and thus a running `evolution-source-registry`
    /// on the session bus. The test VM and CI do not have one. Do not touch
    /// anything through the result. The parent bytes are a valid bit pattern
    /// but not a GObject, so giving them to an EDS function is undefined
    /// behaviour.
    ///
    /// Of the vfuncs below, only `dup_resource_id` is sound on this instance,
    /// because it does not read the backend: its answer comes from the child
    /// source. The other vfuncs use the instance. For example, `populate`
    /// first calls `e_collection_backend_freeze_populate` on it. Thus the
    /// populate *body* is in [`crate::populate`], behind a trait.
    /// `tests/backend.rs` can only check that the slot is installed and is not
    /// the slot of the parent.
    #[cfg(feature = "testing")]
    pub fn detached() -> Box<Self> {
        // SAFETY: each field of the parent is a pointer or an integer, and
        // each field of the watch is an `AtomicBool`. All-zero is a valid
        // value for all of them. For the watch, it is an empty `AccountWatch`.
        unsafe { zeroed_box() }
    }

    /// The [`AccountWatch`] of this instance.
    ///
    /// Reads one field of the half of the instance struct that this crate
    /// owns, and touches no parent bytes. Thus, with `dup_resource_id`, it is
    /// safe to call on a `detached` instance.
    pub fn watch(&self) -> &AccountWatch {
        &self.watch
    }
}

// SAFETY: both structs are #[repr(C)]. They start with the ECollectionBackend
// instance struct and class struct respectively. ECollectionBackend derives
// from GObject (through EBackend).
unsafe impl ObjectSubclass for JmapCollectionBackend {
    const NAME: &'static CStr = c"ECollectionBackendJmap";
    type Instance = JmapCollectionBackend;
    type Class = JmapCollectionBackendClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the EDS type system initialises itself.
        unsafe { e_collection_backend_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points to a newly allocated class struct of this
        // type, which starts with the class struct of the parent. The slots
        // below are in that part.
        let vfuncs = unsafe { &mut (*class).parent_class };
        vfuncs.dup_resource_id = Some(dup_resource_id);
        vfuncs.populate = Some(populate);
        vfuncs.child_added = Some(child_added);
        // The EDS default of these two slots is a *refusal*, not a wrong
        // answer: `collection_backend_create_resource()` and its delete
        // counterpart only return `G_IO_ERROR_NOT_SUPPORTED`. Thus, unlike
        // `child_added`, these overrides must not chain up. Refer to
        // `crate::create_resource` and `crate::delete_resource`.
        vfuncs.create_resource_sync = Some(create_resource_sync);
        vfuncs.delete_resource_sync = Some(delete_resource_sync);
        // A slot of the grandparent, not of the collection backend. The EDS
        // default accepts each account and contacts nothing, so a write to the
        // wrong offset is a silent error. `tests/backend.rs` checks both.
        vfuncs.parent_class.authenticate_sync = Some(authenticate_sync);
        // The other slot of the grandparent. The EDS default reads the
        // "connectable" property, which this crate never sets, so it always
        // returns FALSE. Without this override, the reachability monitor of
        // EDS cannot see the host of a JMAP account. Refer to
        // `get_destination_address` below.
        vfuncs.parent_class.get_destination_address = Some(get_destination_address);
    }
}

/// The class struct of the parent, for the vfuncs that chain up (`populate`
/// and `child_added`).
///
/// `g_type_class_peek`, not `_ref`, as in `jmap-backend-book`. When a vfunc
/// runs on an instance of the subclass, the parent class is initialised. A
/// reference taken here needs a release, and this path has no good place for
/// one.
fn parent_class() -> Option<&'static ECollectionBackendClass> {
    // SAFETY: the contract above.
    unsafe {
        subclass::parent_class::<ECollectionBackendClass>(
            <JmapCollectionBackend as ObjectSubclass>::parent_type(),
        )
    }
}

/// EDS calls this on a cached child source to find its collection, once per
/// file, before `populate`.
///
/// The `backend` argument is not read: the answer comes only from the child
/// source. Thus tests can call it without an instance, or with a detached one.
///
/// A panic becomes NULL, and for NULL, EDS deletes the file. That result is
/// wrong, but there is no alternative: the vfunc has no `GError` and no value
/// that means "ask again". The guard logs a critical, which is the only record
/// of such a bug.
unsafe extern "C" fn dup_resource_id(
    backend: *mut ECollectionBackend,
    child_source: *mut ESource,
) -> *mut gchar {
    let _ = backend;
    guard("dup_resource_id", ptr::null_mut(), || {
        // SAFETY: EDS gives one of its own sources, alive for the call.
        let id_opt = unsafe { resource_id_of(child_source) };
        tracing::debug!(
            resource_id = id_opt.as_deref(),
            "duplicated resource id for child source"
        );
        match id_opt {
            // SAFETY: the duplicate becomes the property of EDS, which puts it
            // in its hash table and frees it with `g_free`.
            Some(id) => unsafe { dup_string(&id) },
            None => ptr::null_mut(),
        }
    })
}

/// EDS calls this to get the children of a collection. It calls it in an idle
/// callback after the account is added, when the user enables or disables a
/// part, and when the backend goes online. [`on_account_changed`] calls it again after an
/// edit of the account, because EDS does not schedule a populate for that.
///
/// [`crate::populate::populate`] makes the decisions. This function reads the
/// account, which a populate does not get, and writes the report.
/// `e_source_registry_debug_print` is the debug channel of EDS. It is off
/// unless `SOURCE_REGISTRY_DEBUG` is set, and `e_collection_backend_new_child`
/// writes its pairing line to it too.
///
/// A panic becomes a logged critical and no populate. The vfunc returns
/// `void`, so it cannot report an error, and the next populate of this account
/// tries again from the same state. The freeze is always released. Refer to
/// [`crate::populate`].
unsafe extern "C" fn populate(backend: *mut ECollectionBackend) {
    guard("populate", (), || {
        // `e_backend_get_source` is `(transfer none)`. It cannot be NULL for a
        // backend that EDS constructed *from* a source. If it is NULL, the
        // cached children are still exported, because that needs no account.
        // No server is contacted, as for an account with no parts enabled.
        // SAFETY: EDS gives one of its own backends, alive for the call, and
        // `ECollectionBackend` derives from `EBackend`.
        let source = unsafe { e_backend_get_source(backend.cast()) };
        let (parts, user, uses_oauth2, account_id, login) = if source.is_null() {
            log_critical("populate: the collection backend has no account source");
            (Parts::NONE, None, false, None, 0)
        } else {
            // SAFETY: a valid `ESource` that the backend owns. This code only
            // reads it.
            let (parts, user, uses_oauth2) = unsafe {
                (
                    parts_of(source),
                    user_of(source),
                    source_uses_oauth2(source),
                )
            };
            // SAFETY: as above. The uid is `(transfer none)`.
            let account_id = unsafe { read_string(e_source_get_uid(source)) };
            // SAFETY: as above.
            let login = unsafe { login_fingerprint(source) };
            (parts, user, uses_oauth2, account_id, login)
        };

        // Do these two steps before the body and in this order, as
        // `ews_backend_populate` does. First, mark the settings unproven until
        // a fan-out succeeds. Then connect the handler once, in the first
        // populate only.
        // SAFETY: EDS dispatched this vfunc on an instance of this type.
        if let Some(watch) = unsafe { watch_of(backend) } {
            watch.populating(login);
            if !source.is_null() && watch.claim_connection() {
                // SAFETY: the account source that EDS owns, alive at least as
                // long as the backend that references it. The backend is the
                // one on which EDS dispatched this vfunc.
                unsafe { connect_account_changed(source, backend) };
            }
        }

        let collection = Live(backend);
        // SAFETY: the methods of `Live` are the EDS calls that `Populating`
        // documents, on a backend that is valid for the duration of the vfunc.
        let report =
            unsafe { crate::populate::populate(&collection, parts, user.as_deref(), uses_oauth2) };
        // `None` means that a different populate of this account is running.
        // That populate does the work.
        let Some(report) = report else { return };

        if report.unidentified > 0 {
            // EDS caches only a source for which `dup_resource_id` gave an id.
            // Thus this branch means that a source changed after it was cached.
            let message = format!(
                "populate: {} cached children of this account could not be named and stay hidden",
                report.unidentified
            );
            match account_id.as_deref() {
                Some(account_id) => log_critical_for_account(account_id, &message),
                None => log_critical(&message),
            }
        }

        let message = format!(
            "populate: {} cached children exported, asked to authenticate: {:?}",
            report.children.len(),
            report.asked
        );
        match account_id.as_deref() {
            Some(account_id) => debug_print_for_account(account_id, &message),
            None => debug_print(&message),
        }
    });
}

/// The [`AccountWatch`] of the instance on which EDS dispatched a vfunc, or
/// `None` for a NULL backend.
///
/// # Safety
///
/// `backend` must be NULL or a live `ECollectionBackendJmap` (the type on
/// which EDS dispatches the vfuncs of this crate).
unsafe fn watch_of(backend: *mut ECollectionBackend) -> Option<&'static AccountWatch> {
    // SAFETY: the contract above. `JmapCollectionBackend` is `#[repr(C)]` and
    // starts with the `ECollectionBackend` that the pointer identifies.
    unsafe { backend.cast::<JmapCollectionBackend>().as_ref() }.map(JmapCollectionBackend::watch)
}

/// Connects [`on_account_changed`] to the `"changed"` signal of the account
/// source, with `backend` as the object.
///
/// `g_signal_connect_object`, not `g_signal_connect_data`. Thus this code keeps
/// no handler id and needs no `dispose` override. [`crate::source_changed`]
/// tells why.
///
/// # Safety
///
/// `source` must be a live `ESource` and `backend` a live
/// `ECollectionBackendJmap`. The connection stays after this call, so `source`
/// must stay alive at least as long as `backend`. It does: `EBackend` holds a
/// reference to this source for the full life of the backend.
unsafe fn connect_account_changed(source: *mut ESource, backend: *mut ECollectionBackend) {
    // SAFETY: the contract above. GLib calls a handler of a signal with no
    // parameters with two arguments: the emitting instance, then the data of
    // the connection. The signature of `on_account_changed` agrees.
    // `tests/source_changed.rs` checks the arity against the linked library.
    unsafe {
        g_signal_connect_object(
            source.cast(),
            c"changed".as_ptr(),
            gcallback(on_account_changed as unsafe extern "C" fn(*mut ESource, gpointer)),
            backend.cast(),
            G_CONNECT_DEFAULT,
        );
    }
}

/// GLib calls this when a setting of the account changes. EDS does not
/// schedule a populate for that. Thus, without this handler, a corrected host
/// or port is not tried until the user enables the account again.
///
/// Runs a populate again, unless a fan-out already succeeded with the current
/// settings of the account. Refer to [`crate::source_changed`].
///
/// # Safety
///
/// The signal emission of GLib calls this with the arguments of an
/// `ESource::changed` handler. The first is the emitting source. It is not
/// used, because this handler uses the account that the backend holds. The
/// second is the backend of the connection. The marshal guards of
/// `g_object_watch_closure` keep the backend referenced for the duration of
/// this call.
unsafe extern "C" fn on_account_changed(source: *mut ESource, backend: gpointer) {
    let _ = source;
    guard("on_account_changed", (), || {
        let backend = backend.cast::<ECollectionBackend>();
        // SAFETY: the backend of the connection, by the contract above.
        let Some(watch) = (unsafe { watch_of(backend) }) else {
            return;
        };
        // The account of the backend, not the emitting source, although they
        // are the same object. The fingerprint that the populate below records
        // must come from the source that the populate reads.
        // SAFETY: EDS gives the backend its source `(transfer none)`, alive as
        // long as the backend. `ECollectionBackend` derives from `EBackend`.
        let account = unsafe { e_backend_get_source(backend.cast()) };
        if account.is_null() {
            return;
        }
        // SAFETY: a valid `ESource` that the backend owns. This code only
        // reads it.
        if !watch.wants_repopulate(unsafe { login_fingerprint(account) }) {
            return;
        }

        // SAFETY: as above. This is the function in the vfunc slot, and the
        // handler of EWS also calls its populate directly.
        unsafe { populate(backend) };
    });
}

/// EDS calls this for each source that appears under this collection. These
/// are the children that a fan-out wrote, the cached children that a populate
/// exported, and the mail sources. This backend does not create or cache the
/// mail sources.
///
/// [`crate::child_added::follow_collection`] makes the decisions. This
/// function gets the account source, which the vfunc does not get, and
/// chains up.
///
/// The chain-up is **first**, the opposite of `e_ews_backend_child_added`.
/// The parent puts the child in the table of the backend and binds its enabled
/// flag to the flag of the account. Thus `e_collection_backend_list_*_sources`
/// knows the child, and the removal pass of the next fan-out reads that list.
/// A panic in the binding below must not prevent this. This order cannot help
/// with a panic in the chain-up, which is EDS code.
///
/// A missing account source gives a logged critical and no binding, as in
/// `populate`. The child is already exported and works with its written
/// settings. It only does not follow the account.
unsafe extern "C" fn child_added(backend: *mut ECollectionBackend, child_source: *mut ESource) {
    guard("child_added", (), || {
        match parent_class().and_then(|class| class.child_added) {
            // SAFETY: the child_added of the parent, called on an instance of
            // a derived type. This is how C chains up.
            Some(child_added) => unsafe { child_added(backend, child_source) },
            // EDS installs its own, so this cannot occur. If it occurs, the
            // binding below is still useful.
            None => log_critical("child_added: the parent class has no child_added to chain up to"),
        }

        // Each child of a collection comes here exactly once: from a fan-out,
        // from the cache in a populate, or from a create. Thus this is the
        // only place that writes the flag. `collection_backend_child_added` in
        // EDS writes `removable = FALSE` at the same point. This is
        // deliberately before the account read below. The permission to
        // delete a child from the server depends only on the child, so a
        // missing account must not remove the menu item.
        // SAFETY: a child source of a collection, alive for the duration of
        // the vfunc. `evolution-source-registry` holds these only as
        // `EServerSideSource`s.
        unsafe { offer_deletion(child_source) };

        // `(transfer none)`. NULL only for a backend that EDS did not
        // construct from a source.
        // SAFETY: EDS gives one of its own backends, alive for the call, and
        // `ECollectionBackend` derives from `EBackend`.
        let source = unsafe { e_backend_get_source(backend.cast()) };
        if source.is_null() {
            // The account is missing, but `child_source` is not, as in the
            // unexported-child branch of `Live::export`. Thus an address book
            // or calendar child still has a resource id for the log.
            // SAFETY: a child source of a collection, alive for the duration
            // of the vfunc, as documented above.
            match unsafe { resource_id_of(child_source) } {
                Some(id) => log_critical_for_resource(
                    &id,
                    "child_added: the collection backend has no account source",
                ),
                None => log_critical("child_added: the collection backend has no account source"),
            }
            return;
        }

        // SAFETY: the account source that EDS owns and one of its child
        // sources, both alive for the duration of the vfunc.
        unsafe { follow_collection(source, child_source) };

        // A delete that was pending when the registry stopped continues here.
        // SAFETY: a child source, alive for the duration of the vfunc.
        if unsafe { jmap_config::pending_delete::read(child_source) }.is_some() {
            // SAFETY: the backend, its account source and a child, all alive
            // for the duration of the vfunc.
            let _ = watch(unsafe { LiveChild::new(backend, source, child_source) });
        }
    });
}

/// EDS calls this when it has the credentials of the account: after a
/// `populate` asks for them, and each time Evolution tries a login again.
///
/// [`crate::authenticate::authenticate_with`] makes the decisions, and
/// [`crate::fan_out::fan_out`] makes the children. This function supplies the
/// instance, which those two do not get, and writes the report of the
/// fan-out. `authenticate_with` decides all steps before the network (which
/// accounts to contact, which failures cause a new password prompt), so tests
/// can check them.
///
/// The `push_credentials` closure also uses the instance. The doc of
/// `authenticate_with` tells why a successful fan-out gives the new
/// credentials to the running address-book and calendar children
/// (`e_collection_backend_authenticate_children`).
///
/// The two certificate out-parameters are deliberately not written. Refer to
/// [`crate::authenticate`] for why this backend never offers a certificate for
/// the user to trust.
///
/// A panic becomes `E_SOURCE_AUTHENTICATION_ERROR` with the error set. Not
/// `REJECTED`: then EDS discards a password that is probably correct and asks
/// for it again on each retry. A bug in this code does not mean that the
/// password is wrong.
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
        // `(transfer none)`. NULL only for a backend that EDS did not
        // construct from a source. For NULL, `authenticate_with` returns an
        // error, not a prompt.
        // SAFETY: EDS gives one of its own backends, alive for the call.
        let source = unsafe { e_backend_get_source(backend) };
        // SAFETY: the instance is an `ECollectionBackend`, on which EDS
        // dispatched this vfunc.
        let collection = Live(backend.cast());

        // SAFETY: the arguments are those of the vfunc, as the contract of
        // `authenticate_with` requires. The methods of `Live` are the EDS
        // calls that `Collection` documents.
        unsafe {
            authenticate_with(
                source,
                credentials,
                cancellable,
                error,
                |login| {
                    let report = fan_out(&collection, &login)?;
                    // SAFETY: EDS dispatched this vfunc on an instance of
                    // this type. (Inside the outer `unsafe` block.)
                    if let Some(watch) = watch_of(backend.cast()) {
                        watch.proven();
                    }
                    // SAFETY: `authenticate_with` calls this closure only
                    // after it confirms that `source` is not NULL. The uid is
                    // `(transfer none)`. (Inside the outer `unsafe` block.)
                    let account_id = read_string(e_source_get_uid(source));
                    report_fan_out(&report, account_id.as_deref());
                    Ok(())
                },
                |credentials| {
                    // SAFETY: `authenticate_with` calls this closure only
                    // after the fan-out above succeeds. It uses the same live
                    // `backend` on which EDS dispatched this vfunc, and the
                    // same `credentials` that this vfunc received. (Inside
                    // the outer `unsafe` block.)
                    e_collection_backend_authenticate_children(collection.0, credentials);
                },
            )
        }
    };

    // SAFETY: `error` is what an EDS vfunc receives: NULL or a pointer to a
    // NULL `GError`. `authenticate_with` sets it only on paths that then
    // return. Thus a panic cannot unwind past a set error into the guard,
    // where one `GError` would overwrite another.
    unsafe {
        guard_value(
            "authenticate_sync",
            error,
            E_SOURCE_AUTHENTICATION_ERROR,
            authenticate,
        )
    }
}

/// EDS calls this to find the host to monitor for the reachability of this
/// account, separately from the general network state.
/// `e_backend_is_destination_reachable` and the periodic reachability monitor
/// of EDS use it. This crate calls neither of them.
///
/// [`jmap_backend_core::source::destination_address`] makes the decision.
/// This function gets the account source, which the vfunc does not get, and
/// writes the two out-parameters. EDS uses them, not a `GError`, to give an
/// answer or no answer.
///
/// A panic becomes `FALSE` with both out-parameters not written. That is the
/// usual "no answer" fallback of [`guard`], and the same answer that a backend
/// with no account source gets without a panic.
unsafe extern "C" fn get_destination_address(
    backend: *mut EBackend,
    host: *mut *mut gchar,
    port: *mut guint16,
) -> gboolean {
    guard("get_destination_address", GFALSE, || {
        // `(transfer none)`. NULL only for a backend that EDS did not
        // construct from a source.
        // SAFETY: EDS gives one of its own backends, alive for the call.
        let source = unsafe { e_backend_get_source(backend) };
        if source.is_null() {
            return GFALSE;
        }

        // SAFETY: a valid account source, alive for the call. This code only
        // reads it.
        let Some((address_host, address_port)) = (unsafe { destination_address(source) }) else {
            return GFALSE;
        };

        tracing::debug!(
            host = address_host.as_str(),
            port = address_port,
            "resolved destination address for collection backend"
        );

        // SAFETY: `host` and `port` are the out-parameters of the vfunc,
        // written only on this success path. The duplicated string becomes
        // the property of EDS, which frees it with `g_free`.
        unsafe {
            *host = dup_string(&address_host);
            *port = address_port;
        }
        GTRUE
    })
}

/// EDS calls this when the user asks Evolution for a new address book or
/// calendar *in this account*. It is the only request that creates something
/// on the server, not a mirror of what is there.
///
/// [`crate::create_resource`] and [`jmap_collection_sync::create`] make the
/// decisions. Only a live instance has four values: the account source, its
/// uid, the cache directory of the collection and the registry server. This
/// function supplies them in the necessary order.
///
/// **No chain-up.** The parent gives the `G_IO_ERROR_NOT_SUPPORTED` that this
/// override replaces. A call to it after a successful create reports a
/// failure. [`crate::create_resource`] tells where the EDS source has this.
///
/// The publish is last, and always occurs after the source is written. The EDS
/// documentation of the vfunc makes the addition of the `ESource` to the
/// server part of the contract. Without it, a successful create shows no
/// result until the next populate. Unlike in [`crate::fan_out`], the publish
/// does not depend on `e_collection_backend_is_new_source`. That check is
/// about a child from the cache of the backend. This source comes from the
/// `remote_create` handler of EDS, which never uses the cache.
///
/// A panic becomes `FALSE` with the error set, as the contract of
/// [`guard_bool`](jmap_backend_core::trampoline::guard_bool) gives. This is
/// correct here. The collection can exist on the server or not, and the next
/// populate corrects the state. The same occurs for a create whose source write
/// failed.
unsafe extern "C" fn create_resource_sync(
    backend: *mut ECollectionBackend,
    scratch_source: *mut ESource,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    let create = || {
        // Read the kind and the name from the scratch source that EDS built
        // from the keyfile that Evolution sent. For a source with no kind, the
        // EDS documentation gives "cannot be determined without ambiguity".
        // SAFETY: EDS gives the scratch source that it created, alive for the
        // call.
        let Some(requested) = (unsafe { requested_of(scratch_source) }) else {
            // SAFETY: `error` is what an EDS vfunc receives, and nothing has
            // written to it.
            return unsafe { fail_bool(error, &CreateError::UnknownKind, CreateError::to_gerror) };
        };

        // `(transfer none)`. NULL only for a backend that EDS did not
        // construct from a source. Then there is no account to read a server
        // from and no uid to be the parent of the child. Thus this is an error
        // here, not the logged critical of `populate`.
        // SAFETY: EDS gives one of its own backends, alive for the call, and
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

        // SAFETY: a live backend, a valid account source and a cancellable
        // that is NULL or from EDS, as the contract of `login_for` requires.
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

        // Held for the duration of the create only, as in `authenticate_sync`.
        // SAFETY: `cancellable` is NULL or a valid `GCancellable` that EDS
        // keeps alive for the duration of the vfunc.
        let _cancel = unsafe { observe(cancellable) };

        let child = match create_on_server(
            &login.server.target,
            login.server.rebase_urls,
            login.credentials.clone(),
            &requested,
        ) {
            Ok(child) => child,
            // SAFETY: as above.
            Err(failure) => {
                return unsafe { fail_bool(error, &failure, CreateError::to_gerror) };
            }
        };

        // SAFETY: a valid account source. Both getters return NULL or a string
        // that the object owns.
        let (account_uid, cache_dir) = unsafe {
            (
                read_string(e_source_get_uid(account)),
                read_string(e_collection_backend_get_cache_dir(backend)),
            )
        };
        let Some(account_uid) = account_uid else {
            // A source with no uid cannot be a parent. Each source that the
            // registry loads has one: its file name.
            // SAFETY: as above.
            return unsafe {
                fail_invalid(
                    error,
                    "the JMAP collection backend's account has no uid to parent a new \
                     collection to",
                )
            };
        };

        // SAFETY: the scratch source is the `EServerSideSource` that the
        // `remote_create` handler of EDS built, alive for the call.
        if let Err(setting) = unsafe {
            adopt_created(
                scratch_source,
                &child,
                &login.server.connection,
                &account_uid,
                cache_dir.as_deref(),
            )
        } {
            // The collection is on the server, but this child is not written,
            // so nothing is exported. `adopt_created` tells why this is
            // correct and what the next populate does.
            // SAFETY: as above.
            return unsafe {
                fail_bool(
                    error,
                    &CreateError::Unwritable(child.kind, setting),
                    CreateError::to_gerror,
                )
            };
        }

        // The second of the two documented EDS obligations. Evolution does not
        // see a created resource that is not added to the server until the
        // next populate. Then the create seems to have no result.
        Collection::publish(&Live(backend), scratch_source);
        debug_print_for_resource(
            &child.resource_id,
            &format!(
                "create_resource_sync: created {} {}",
                kind_noun(child.kind),
                child.resource_id
            ),
        );
        GTRUE
    };

    // SAFETY: `error` is what an EDS vfunc receives: NULL or a pointer to a
    // NULL `GError`. Each path above that sets it then returns. Thus a panic
    // cannot unwind past a set error into the guard.
    unsafe { guard_bool("create_resource_sync", error, create) }
}

/// EDS calls this when the user selects "Delete" on an address book or a
/// calendar of this account. It is the only request that destroys something
/// on the server.
///
/// [`crate::delete_resource`] and [`jmap_collection_sync::delete`] make the
/// decisions. Only a live instance has two values: the account source and,
/// through it, the credentials of the account. This function supplies them
/// and puts the three steps in order.
///
/// **No chain-up**, for the reason that `create_resource_sync` gives: the
/// parent gives the `G_IO_ERROR_NOT_SUPPORTED` that this override replaces.
///
/// **Nothing goes to the server here.** The vfunc disables the child and
/// stores a due time on it. A [`LiveChild`] thread destroys the collection and
/// then removes the source at that time. Refer to [`crate::deferred_delete`].
///
/// A panic becomes `FALSE` with the error set.
unsafe extern "C" fn delete_resource_sync(
    backend: *mut ECollectionBackend,
    child_source: *mut ESource,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // Nothing here waits, so there is nothing to cancel.
    let _ = cancellable;
    let delete = || {
        // The collection of this source, read as `dup_resource_id` reads it.
        // A source with no answer does not belong to this backend, so this
        // code must destroy nothing for it.
        // SAFETY: EDS gives one of its own sources, alive for the call.
        let Some(doomed) = (unsafe { doomed_of(child_source) }) else {
            // SAFETY: `error` is what an EDS vfunc receives, and nothing has
            // written to it.
            return unsafe { fail_bool(error, &DeleteError::NotOurs, DeleteError::to_gerror) };
        };

        // `(transfer none)`. NULL only for a backend that EDS did not
        // construct from a source. Then there is no account to read a server
        // from, so this is an error here, not the logged critical of
        // `populate`.
        // SAFETY: EDS gives one of its own backends, alive for the call, and
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

        // SAFETY: the backend, its account source and a child of this
        // collection, all alive for the call.
        let due = unsafe {
            let live = LiveChild::new(backend, account, child_source);
            delete_later(child_source, live, SystemTime::now())
        };

        debug_print_for_resource(
            doomed.collection_id.as_str(),
            &format!(
                "delete_resource_sync: the destroy of {} {} is due at {}",
                kind_noun(doomed.kind),
                doomed.collection_id,
                due.stamp()
            ),
        );
        GTRUE
    };

    // SAFETY: as in `create_resource_sync`. Each path above that sets the
    // error then returns, so a panic cannot unwind past a set error.
    unsafe { guard_bool("delete_resource_sync", error, delete) }
}

/// A child with a pending delete, held by a [`watch`] thread.
///
/// It holds a reference to the backend and to the child. The backend holds the
/// account source for its full life.
struct LiveChild {
    backend: *mut ECollectionBackend,
    account: *mut ESource,
    child: *mut ESource,
}

// SAFETY: the reference counts of GObject are atomic. EDS already uses this
// backend and its children from other threads: it runs `authenticate_sync` and
// `delete_resource_sync` in a `GTask` thread.
unsafe impl Send for LiveChild {}

impl LiveChild {
    /// Takes a new reference to `backend` and to `child`.
    ///
    /// # Safety
    ///
    /// `backend` must be a live `ECollectionBackendJmap`, `account` its account
    /// source and `child` a child source of it.
    unsafe fn new(
        backend: *mut ECollectionBackend,
        account: *mut ESource,
        child: *mut ESource,
    ) -> Self {
        // SAFETY: two live GObjects, by the contract of this function.
        unsafe {
            g_object_ref(backend.cast());
            g_object_ref(child.cast());
        }
        Self {
            backend,
            account,
            child,
        }
    }
}

impl Drop for LiveChild {
    fn drop(&mut self) {
        // SAFETY: the two references that `new` took.
        unsafe {
            g_object_unref(self.child.cast());
            g_object_unref(self.backend.cast());
        }
    }
}

// SAFETY for each method: `new` holds a reference to the backend and the
// child, and the backend holds its account source.
impl PendingChild for LiveChild {
    fn uid(&self) -> String {
        // SAFETY: a live source. The uid is `(transfer none)`.
        unsafe { read_string(e_source_get_uid(self.child)) }.unwrap_or_default()
    }

    fn present(&self) -> bool {
        // SAFETY: each child of a collection is an `EServerSideSource`.
        unsafe { e_server_side_source_get_exported(self.child.cast()) != GFALSE }
    }

    fn enabled(&self) -> bool {
        // SAFETY: a live source.
        unsafe { e_source_get_enabled(self.child) != GFALSE }
    }

    fn due(&self) -> Option<jmap_collection_sync::pending::Due> {
        // SAFETY: a live source.
        unsafe { jmap_config::pending_delete::read(self.child) }
    }

    fn clear_due(&self) {
        // SAFETY: a live source.
        unsafe { jmap_config::pending_delete::write(self.child, None) };
    }

    fn enable(&self) {
        // SAFETY: a live source.
        unsafe { e_source_set_enabled(self.child, GTRUE) };
    }

    fn destroy_on_server(&self) -> Result<(), DeleteError> {
        // SAFETY: a live source.
        let doomed = unsafe { doomed_of(self.child) }.ok_or(DeleteError::NotOurs)?;
        // SAFETY: a live backend and its account source. No cancellable.
        let login = unsafe {
            login_for(
                self.backend,
                self.account,
                ptr::null_mut(),
                "pending delete",
            )
        }?;
        delete_on_server(
            &login.server.target,
            login.server.rebase_urls,
            login.credentials.clone(),
            &doomed,
        )
    }

    fn remove(&self) -> Result<(), String> {
        // SAFETY: a child source of this collection. No cancellable.
        unsafe { remove_source(self.child, ptr::null_mut()) }
    }
}

/// The [`Login`] of the account, for the two vfuncs that get no
/// `ENamedParameters` from EDS.
///
/// `authenticate_sync` gets the credentials from EDS. `create_resource_sync`
/// and `delete_resource_sync` do not, so they must ask for them.
/// [`crate::create_resource`] tells why this code asks the store when it needs
/// the password, and does not keep a secret for the life of the account. An
/// OAuth 2.0 account ignores the password and gets a new token in
/// [`login_of`].
///
/// `context` names the calling vfunc, for the critical channel.
///
/// # Safety
///
/// `backend` must be a valid `ECollectionBackend`, `account` the valid
/// `ESource` from which EDS constructed it, and `cancellable` NULL or a valid
/// `GCancellable`. Both vfuncs receive these.
unsafe fn login_for(
    backend: *mut ECollectionBackend,
    account: *mut ESource,
    cancellable: *mut GCancellable,
    context: &str,
) -> Result<Login, crate::authenticate::LoginError> {
    // SAFETY: a valid backend, by the contract of this function. The server is
    // `(transfer full)`. The drop at the end of the scope releases the
    // reference.
    let password = unsafe {
        let server =
            Owned::<ESourceRegistryServer>::from_raw(e_collection_backend_ref_server(backend));
        let server_ptr = server.as_ref().map_or(ptr::null_mut(), Owned::as_ptr);
        stored_password_of(server_ptr, account, cancellable, context)
    };

    // SAFETY: a valid account source and a cancellable that obey the contract
    // of `login_of`, because the contract of this function requires them.
    unsafe { login_of(account, parts_of(account), password.as_deref(), cancellable) }
}

/// Reports the result of one fan-out on the two channels that remain after
/// the vfunc has returned its answer.
///
/// The three failure lists are criticals. Each entry is a child that the user
/// asked for and will not see, and the user cannot fix it. `uncreated` is a
/// resource that EDS refused to claim. `abandoned` is a setting that this
/// crate cannot write. `not_removed` is a deletion that EDS refused. The
/// successes go to the debug channel of EDS, which is silent unless
/// `SOURCE_REGISTRY_DEBUG` is set.
///
/// None of this goes to the `GError`. A successful login is `ACCEPTED`, also
/// if one address book could not be written. A failed authentication for one
/// child failure would take the full account offline because of one
/// collection.
///
/// `account_id` names the account of this fan-out. Only the summary line
/// below has it. Each entry of the three lists has its own `resource_id`,
/// which is a more specific structured field.
fn report_fan_out(report: &Populated, account_id: Option<&str>) {
    for resource_id in &report.uncreated {
        log_critical_for_resource(
            resource_id,
            &format!("authenticate_sync: EDS would not create a child source for {resource_id}"),
        );
    }
    for abandoned in &report.abandoned {
        log_critical_for_resource(
            &abandoned.resource_id,
            &format!(
                "authenticate_sync: {} stays unexported: {}",
                abandoned.resource_id, abandoned.setting
            ),
        );
    }
    for not_removed in &report.not_removed {
        log_critical_for_resource(
            &not_removed.resource_id,
            &format!(
                "authenticate_sync: {} should have been removed and was not: {}",
                not_removed.resource_id, not_removed.message
            ),
        );
    }

    let message = format!(
        "authenticate_sync: {} children written",
        report.children.len()
    );
    match account_id {
        Some(account_id) => debug_print_for_account(account_id, &message),
        None => debug_print(&message),
    }
}

/// A live collection, as a populate and a fan-out use it: the EDS calls behind
/// [`Populating`] and [`Collection`], one line each.
///
/// Tests cannot run this part without `evolution-source-registry`. Refer to
/// [`crate::populate`] and [`crate::fan_out`] for why all other code is one
/// layer down.
struct Live(*mut ECollectionBackend);

impl Live {
    /// `e_source_registry_server_add_source (server, child)`. Both traits call
    /// it `publish`. It is a call on the server, not on the backend.
    ///
    /// This takes a server reference for each child, not one for a loop.
    /// Neither trait has a setup call that can hold one, and a fan-out exports
    /// only a small number of children. `e_collection_backend_ref_server` reads
    /// a weak reference, so NULL means that the registry server is gone (for
    /// example, during shutdown). Then there is nothing to export to.
    ///
    /// # Safety
    ///
    /// `child` must be a valid `ESource` that outlives the call. Each caller
    /// publishes only a child whose `[Resource] Identity` is already written:
    /// `populate` checks [`resource_id_of`] before this call, `fan_out::adopt`
    /// publishes only after its settings write succeeds, and
    /// `create_resource_sync` publishes only after `adopt_created` succeeds.
    /// Thus the failure branch below can read the identity for the log.
    fn export(&self, child: *mut ESource, context: &str) {
        // SAFETY: a valid backend. The server is `(transfer full)`.
        let server = unsafe {
            Owned::<ESourceRegistryServer>::from_raw(e_collection_backend_ref_server(self.0))
        };
        let Some(server) = server else {
            let message =
                format!("{context}: the registry server is gone; a child stays unexported");
            // SAFETY: `child` is a valid `ESource`, by the contract of this
            // function.
            match unsafe { resource_id_of(child) } {
                Some(resource_id) => log_critical_for_resource(&resource_id, &message),
                None => log_critical(&message),
            }
            return;
        };

        // SAFETY: a live registry server and a live child source. If the call
        // keeps the source, it takes its own reference.
        unsafe { e_source_registry_server_add_source(server.as_ptr(), child) };
    }
}

/// The `ESource`s of a `(transfer full)` `GList`. The list is freed, and each
/// reference in it goes to the caller.
///
/// `g_list_free`, not `_full`, as `e_collection_backend_claim_all_resources()`
/// and `e_collection_backend_list_*_sources()` document, and as
/// [`Populating`] and [`Collection`] promise their callers.
///
/// # Safety
///
/// `list` must be NULL or a `GList` that this call can free, whose `data` are
/// `ESource *` with one reference each.
unsafe fn drain(list: *mut GList) -> Vec<*mut ESource> {
    let mut sources = Vec::new();
    let mut node = list;
    while !node.is_null() {
        // SAFETY: a live node of the list, whose `data` is one of the sources
        // referenced for this code.
        sources.push(unsafe { (*node).data }.cast::<ESource>());
        // SAFETY: as above.
        node = unsafe { (*node).next };
    }

    // SAFETY: this call owns the list, and nothing else holds its nodes.
    unsafe { g_list_free(list) };

    sources
}

// SAFETY: `claim_all_resources` returns what
// `e_collection_backend_claim_all_resources` referenced for this code, one
// reference for each source. The freeze and thaw are the two halves of the EDS
// counter.
unsafe impl Populating for Live {
    fn freeze(&self) -> bool {
        // SAFETY: a valid backend for the duration of the enclosing vfunc.
        unsafe { e_collection_backend_freeze_populate(self.0) != GFALSE }
    }

    fn thaw(&self) {
        // SAFETY: as above.
        unsafe { e_collection_backend_thaw_populate(self.0) };
    }

    fn chain_up(&self) {
        match parent_class().and_then(|class| class.populate) {
            // SAFETY: the populate of the parent, called on an instance of a
            // derived type. This is how C chains up.
            Some(populate) => unsafe { populate(self.0) },
            // EDS installs a placeholder, so this cannot occur. If it occurs,
            // the populate below is still useful.
            None => log_critical("populate: the parent class has no populate to chain up to"),
        }
    }

    fn claim_all_resources(&self) -> Vec<*mut ESource> {
        // SAFETY: as above. The claim is `(transfer full)`, one reference for
        // each source, and `drain` gives these to the caller.
        unsafe { drain(e_collection_backend_claim_all_resources(self.0)) }
    }

    fn publish(&self, child: *mut ESource) {
        self.export(child, "populate");
    }

    fn request_credentials(&self) {
        // No certificate and no error. This backend never gives EDS a
        // certificate to offer the user (refer to `crate::authenticate`). No
        // operation failed: this is the first request to the account.
        // SAFETY: a valid backend, which derives from `EBackend`. Each other
        // argument is optional and NULL, and the reason is a value of the enum.
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
        // SAFETY: as above. NULL credentials make an anonymous authenticate,
        // and `jmap_backend_core::marshal::password` reads them as absent.
        unsafe { e_backend_schedule_authenticate(self.0.cast(), ptr::null()) };
    }

    fn offer_creation(&self, offer: bool) {
        // The account source, not a child. Evolution reads this flag to
        // decide if "New Address Book" can use this account.
        //
        // In `populate`, not in a `constructed` override as in evolution-ews.
        // This crate has no `constructed`. One would need a slot in
        // `GObjectClass`, a third class struct above the `EBackendClass` of
        // `authenticate_sync`, for a one-line effect. EDS calls `populate`
        // first on a collection backend, and again on each reconnect and each
        // account change. A flag that comes from the account settings needs
        // exactly that frequency.
        // SAFETY: a valid backend for the duration of the enclosing vfunc.
        // `e_backend_get_source` is `(transfer none)`. The account source of a
        // backend that the registry constructed is an `EServerSideSource`. A
        // different source gets the `g_return_if_fail` critical of EDS, not
        // undefined behaviour. For NULL, this does nothing, because a backend
        // with no account has no flag to write.
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

    fn allow_rename(&self) {
        // SAFETY: as in `offer_creation`: a valid backend,
        // `e_backend_get_source` is `(transfer none)`, and for NULL this does
        // nothing. If the account source has no `Collection` extension,
        // `e_source_get_extension` creates one and does not return NULL. Thus
        // the cast below always gives a live `ESourceCollection`.
        unsafe {
            let source = e_backend_get_source(self.0.cast());
            if source.is_null() {
                return;
            }
            let collection: *mut ESourceCollection =
                e_source_get_extension(source, E_SOURCE_EXTENSION_COLLECTION.as_ptr()).cast();
            e_source_collection_set_allow_sources_rename(collection, GTRUE);
        }
    }
}

// SAFETY: `new_child` and `existing_children` both return what EDS referenced
// for this code, one reference for each source. Both are `(transfer full)`, as
// `Collection` requires.
unsafe impl Collection for Live {
    fn new_child(&self, resource_id: &str) -> *mut ESource {
        // `CString::new`, not `cstring_lossy`. A cut at an interior NUL does
        // not fail. It silently asks EDS for a *different* resource and pairs
        // the child of this collection with it. No child exists for a resource
        // id that EDS cannot receive. NULL means that here, and `adopt`
        // reports it as `Uncreated`. Only a server that puts a NUL in an id can
        // cause this. Each other resource id in this crate comes from a C
        // string.
        let Ok(resource_id) = CString::new(resource_id) else {
            return ptr::null_mut();
        };
        // SAFETY: a valid backend for the duration of the enclosing vfunc, and
        // a NUL-terminated string that the call only reads.
        unsafe { e_collection_backend_new_child(self.0, resource_id.as_ptr()) }
    }

    fn is_new_child(&self, child: *mut ESource) -> bool {
        // SAFETY: a valid backend and one of the child sources that it
        // returned, both alive for this call.
        unsafe { e_collection_backend_is_new_source(self.0, child) != GFALSE }
    }

    fn publish(&self, child: *mut ESource) {
        self.export(child, "authenticate_sync");
    }

    fn existing_children(&self) -> Vec<*mut ESource> {
        // Address books and calendars, in that order, and never mail. This
        // backend creates no mail children, so it must not remove them. Two
        // calls, because the EDS accessor for all children also gives each
        // mail source.
        // SAFETY: a valid backend. Each list is `(transfer full)`, one
        // reference for each source, and `drain` gives these to the caller.
        let mut children = unsafe { drain(e_collection_backend_list_contacts_sources(self.0)) };
        // SAFETY: as above.
        children.extend(unsafe { drain(e_collection_backend_list_calendar_sources(self.0)) });
        children
    }

    fn set_remote_deletable(&self, child: *mut ESource, deletable: bool) {
        // SAFETY: each source on which `adopt` calls this comes from
        // `e_collection_backend_new_child`. That function returns an
        // `EServerSideSource`, as for each child of a collection in
        // `evolution-source-registry`. `delete_resource::offer_deletion` makes
        // the same `.cast()` from the same guarantee.
        unsafe {
            e_server_side_source_set_remote_deletable(
                child.cast(),
                if deletable { GTRUE } else { GFALSE },
            )
        };
    }
}

/// Writes one line on the debug channel of EDS, which is silent unless
/// `SOURCE_REGISTRY_DEBUG` is set, and on the DEBUG channel of `tracing`.
/// Thus the write and discovery events also go to journald at DEBUG, after a
/// module calls [`jmap_backend_core::logging::init`]. The callers of
/// `log_critical` send their events at ERROR.
fn debug_print(message: &str) {
    tracing::debug!("{message}");
    debug_print_eds(message);
}

/// Like [`debug_print`], but also attaches `resource_id` as a structured
/// `tracing` field, as
/// [`jmap_backend_core::trampoline::log_critical_for_resource`] does at ERROR
/// level. For the two vfuncs (`create_resource_sync`, `delete_resource_sync`)
/// that have the resource id when they report success. The EDS debug channel
/// does not change: its text also contains the id.
fn debug_print_for_resource(resource_id: &str, message: &str) {
    tracing::debug!(resource_id, "{message}");
    debug_print_eds(message);
}

/// Like [`debug_print`], but also attaches `account_id` as a structured
/// `tracing` field, as [`log_critical_for_account`] does at ERROR level. For a
/// `populate` or fan-out summary about all children of an account, which has
/// no single resource to name. The EDS debug channel does not change.
fn debug_print_for_account(account_id: &str, message: &str) {
    tracing::debug!(account_id, "{message}");
    debug_print_eds(message);
}

/// The EDS debug-channel part that [`debug_print`],
/// [`debug_print_for_resource`] and [`debug_print_for_account`] share.
fn debug_print_eds(message: &str) {
    let message = cstring_lossy(message);
    // SAFETY: `e_source_registry_debug_print` is variadic and takes a printf
    // format. The text is an argument to `%s`, so a `%` in it is not read as a
    // directive.
    unsafe { e_source_registry_debug_print(c"%s\n".as_ptr(), message.as_ptr()) };
}

#[cfg(test)]
mod report_fan_out_tests {
    use jmap_tracing_capture::capture;

    use super::*;
    use crate::child_source::UnwritableSetting;
    use crate::fan_out::Abandoned;
    use crate::removal::NotRemoved;

    #[test]
    fn each_failure_list_logs_a_critical_naming_its_own_resource() {
        let report = Populated {
            children: Vec::new(),
            uncreated: vec!["addressbook-1".to_owned()],
            abandoned: vec![Abandoned {
                resource_id: "addressbook-2".to_owned(),
                setting: UnwritableSetting::UnknownProperty {
                    group: "Authentication",
                    key: "Host",
                },
            }],
            not_removed: vec![NotRemoved {
                resource_id: "addressbook-3".to_owned(),
                message: "refused".to_owned(),
            }],
        };

        let (_, entries) = capture(|| report_fan_out(&report, None));

        for resource_id in ["addressbook-1", "addressbook-2", "addressbook-3"] {
            assert!(
                entries
                    .iter()
                    .any(|(_, name, value)| name == "resource_id" && value == resource_id),
                "expected a resource_id={resource_id} field, got {entries:?}"
            );
        }
    }

    #[test]
    fn the_summary_carries_the_account_id_only_when_one_is_given() {
        let report = Populated::default();

        let (_, with_account) = capture(|| report_fan_out(&report, Some("account-1")));
        assert!(
            with_account
                .iter()
                .any(|(_, name, value)| name == "account_id" && value == "account-1"),
            "expected an account_id=account-1 field, got {with_account:?}"
        );

        let (_, without_account) = capture(|| report_fan_out(&report, None));
        assert!(
            without_account
                .iter()
                .all(|(_, name, _)| name != "account_id"),
            "did not expect an account_id field, got {without_account:?}"
        );
    }
}

#[cfg(test)]
mod parent_class_tests {
    use super::*;

    #[test]
    fn parent_class_finds_the_class_once_it_is_initialised() {
        // SAFETY: a reference to the type initialises its class if necessary.
        // The code below releases the reference.
        let class = unsafe { gobject_sys::g_type_class_ref(e_collection_backend_get_type()) };
        assert!(
            parent_class().is_some(),
            "parent_class found no class for an initialised type"
        );
        // SAFETY: releases the reference taken above.
        unsafe { gobject_sys::g_type_class_unref(class) };
    }
}

#[cfg(test)]
mod debug_print_tests {
    use jmap_tracing_capture::capture;

    use super::*;

    #[test]
    fn debug_print_for_resource_attaches_the_resource_id_as_a_structured_field() {
        let (_, entries) = capture(|| {
            debug_print_for_resource("addressbook-1", "created address book addressbook-1");
        });

        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "resource_id" && value == "addressbook-1"),
            "expected a resource_id=addressbook-1 field, got {entries:?}"
        );
    }

    #[test]
    fn debug_print_for_account_attaches_the_account_id_as_a_structured_field() {
        let (_, entries) = capture(|| {
            debug_print_for_account("account-1", "3 cached children exported");
        });

        assert!(
            entries
                .iter()
                .any(|(_, name, value)| name == "account_id" && value == "account-1"),
            "expected an account_id=account-1 field, got {entries:?}"
        );
    }

    #[test]
    fn debug_print_does_not_attach_a_resource_id_field() {
        let (_, entries) = capture(|| {
            debug_print("no resource id in this one");
        });

        assert!(!entries.is_empty(), "expected a debug entry, got none");
        assert!(
            entries.iter().all(|(_, name, _)| name != "resource_id"),
            "did not expect a resource_id field, got {entries:?}"
        );
    }
}
