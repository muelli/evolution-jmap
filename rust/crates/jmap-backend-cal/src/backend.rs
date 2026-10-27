// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `ECalMetaBackend` subclass for JMAP calendars.
//! Bridges EDS calendar factory requests to [`ops`] and [`connect`] operations.

use std::ffi::CStr;
use std::ops::Deref;
use std::ptr;
use std::sync::{Mutex, RwLock};

use eds_sys::{
    E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND, E_CAL_OBJ_MOD_ALL, E_CAL_OBJ_MOD_THIS,
    E_CLIENT_ERROR_REPOSITORY_OFFLINE, EBackend, ECalBackend, ECalBackendSync, ECalComponent,
    ECalMetaBackend, ECalMetaBackendClass, ECalOperationFlags, EConflictResolution, EDataCal,
    ENamedParameters, ESourceAuthenticationResult, GTlsCertificateFlags, ICalComponent,
    e_backend_get_source, e_cal_backend_set_writable, e_cal_backend_sync_modify_objects,
    e_cal_cache_get_component, e_cal_cache_get_component_extra, e_cal_client_error_create,
    e_cal_component_get_as_string, e_cal_meta_backend_ensure_connected_sync,
    e_cal_meta_backend_get_type, e_cal_meta_backend_ref_cache, e_cal_meta_backend_schedule_refresh,
    e_cal_util_set_alarm_acknowledged, e_client_error_create, e_util_free_nullable_object_slist,
    time_t,
};
use gio_sys::GCancellable;
use glib_sys::{
    GError, GFALSE, GSList, GTRUE, GType, g_free, g_slist_free_full, g_slist_prepend, gboolean,
    gchar, guint16,
};
use gobject_sys::g_object_unref;
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::{cstring_lossy, set_raw_gerror};
use jmap_backend_core::instance::Slot;
#[cfg(feature = "testing")]
use jmap_backend_core::instance::zeroed_box;
use jmap_backend_core::marshal::{dup_string, read_string};
pub use jmap_backend_core::meta::MetaState;
use jmap_backend_core::oauth2::{access_token, source_uses_oauth2};
use jmap_backend_core::poison::{read, write};
use jmap_backend_core::push::{self, PushRefresh};
use jmap_backend_core::retry::retry_on_authentication_failure;
use jmap_backend_core::source::{backend_source, destination_address};
use jmap_backend_core::subclass::{self, ObjectSubclass};
use jmap_backend_core::trampoline::{guard, guard_bool, guard_value};
use jmap_cal_sync::CalSync;
use jmap_client::Credentials;

use crate::connect::{self, ACCEPTED_AUTH_RESULT, write_auth_result};
use crate::marshal;
use crate::ops::{self, Outcome};
use crate::scheduling::{self, CAPABILITIES_PROPERTY};

/// The JMAP calendar backend.
#[repr(C)]
pub struct JmapCalBackend {
    /// GObject parent instance.
    parent: ECalMetaBackend,
    /// Connection and push lifecycle state.
    pub state: MetaState<CalSync>,
    /// Baseline color tracked for detecting local changes in `source_changed`.
    last_known_color: Slot<RwLock<Option<String>>>,
}

/// Class struct holding vfunc pointers.
#[repr(C)]
pub struct JmapCalBackendClass {
    pub parent_class: ECalMetaBackendClass,
}

impl Deref for JmapCalBackend {
    type Target = MetaState<CalSync>;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl JmapCalBackend {
    /// Creates a detached test instance without GObject initialization.
    #[cfg(feature = "testing")]
    pub fn detached() -> Box<Self> {
        // SAFETY: Parent fields are zeroable integers or pointers. Zeroed Slot is empty.
        let backend: Box<Self> = unsafe { zeroed_box() };
        backend.state.session.init(RwLock::new(None));
        backend.last_known_color.init(RwLock::new(None));
        backend.state.push.init(Mutex::new(None));
        backend
    }

    /// Runs `f` against the active connection under a read lock (testing only).
    #[cfg(feature = "testing")]
    pub fn inspect_connection<R>(&self, f: impl FnOnce(&CalSync) -> R) -> Option<R> {
        let session = self.session()?;
        let guard = read(session);
        guard.as_ref().map(f)
    }

    /// Accesses the session slot if initialized.
    fn session(&self) -> Option<&RwLock<Option<CalSync>>> {
        self.state.session.get()
    }

    /// Accesses the last known color slot if initialized.
    pub fn last_known_color(&self) -> Option<&RwLock<Option<String>>> {
        self.last_known_color.get()
    }
}

// SAFETY: Both structs are #[repr(C)] and start with ECalMetaBackend structs.
// ECalMetaBackend derives from GObject.
unsafe impl ObjectSubclass for JmapCalBackend {
    const NAME: &'static CStr = c"ECalBackendJmap";
    type Instance = JmapCalBackend;
    type Class = JmapCalBackendClass;

    fn parent_type() -> GType {
        // SAFETY: Initializing the GType requires no preconditions.
        unsafe { e_cal_meta_backend_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points to an allocated class struct containing parent fields.
        let vfuncs = unsafe { &mut (*class).parent_class };
        vfuncs.connect_sync = Some(connect_sync);
        vfuncs.disconnect_sync = Some(disconnect_sync);
        vfuncs.list_existing_sync = Some(list_existing_sync);
        vfuncs.get_changes_sync = Some(get_changes_sync);
        vfuncs.load_component_sync = Some(load_component_sync);
        vfuncs.save_component_sync = Some(save_component_sync);
        vfuncs.remove_component_sync = Some(remove_component_sync);
        vfuncs.source_changed = Some(source_changed);

        // Overrides parent property lookup to append scheduling capabilities.
        vfuncs.parent_class.parent_class.impl_get_backend_property = Some(get_backend_property);

        // Overrides parent get_free_busy_sync slot.
        vfuncs.parent_class.get_free_busy_sync = Some(get_free_busy_sync);

        // Overrides parent discard_alarm_sync to forward dismissed alarms to the server.
        vfuncs.parent_class.discard_alarm_sync = Some(discard_alarm_sync);

        // Overrides parent receive_objects_sync to mark received changes.
        vfuncs.parent_class.receive_objects_sync = Some(receive_objects_sync);

        // Installs host reachability callback on EBackendClass.
        vfuncs
            .parent_class
            .parent_class
            .parent_class
            .get_destination_address = Some(get_destination_address);
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // SAFETY: `instance` points to an allocated instance struct with empty slots.
        unsafe {
            (*instance).state.session.init(RwLock::new(None));
            (*instance).last_known_color.init(RwLock::new(None));
            (*instance).state.push.init(Mutex::new(None));
        }
    }

    unsafe fn finalize(instance: *mut Self::Instance) {
        // Stop push worker thread before dropping connection resources.
        // SAFETY: Instance is finalizing and no active references remain.
        unsafe { (*instance).state.push.clear() };
        // SAFETY: Instance is finalizing and no active references remain.
        unsafe {
            (*instance).state.session.clear();
            (*instance).last_known_color.clear();
        }
    }
}

/// Returns the parent class struct for chaining up vfuncs.
pub fn parent_class() -> Option<&'static ECalMetaBackendClass> {
    // SAFETY: Resolves parent class pointer from registered GType.
    unsafe {
        subclass::parent_class::<ECalMetaBackendClass>(
            <JmapCalBackend as ObjectSubclass>::parent_type(),
        )
    }
}

// ---------------------------------------------------------------------------
// the vfunc slots

unsafe extern "C" fn connect_sync(
    meta_backend: *mut ECalMetaBackend,
    credentials: *const ENamedParameters,
    out_auth_result: *mut ESourceAuthenticationResult,
    // Certificate out-parameters are left untouched; TLS errors surface via transport failure.
    _out_certificate_pem: *mut *mut gchar,
    _out_certificate_errors: *mut GTlsCertificateFlags,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: `meta_backend` is a valid instance pointer, out-parameters are writable, and cancellable is valid or null.
    unsafe {
        guard_bool("connect_sync", error, || {
            let Some(backend) = instance(meta_backend) else {
                return fail_offline(error);
            };

            // Avoid re-opening an already active connection.
            if backend.is_connected() {
                tracing::debug!("reusing existing calendar connection");
                write_auth_result(out_auth_result, ACCEPTED_AUTH_RESULT);
                return GTRUE;
            }

            let source = e_backend_get_source(meta_backend.cast());
            let Some(open) =
                connect::connect(source, credentials, cancellable, out_auth_result, error)
            else {
                return GFALSE;
            };

            // The server sends invitations; each change can still suppress its own message.
            let sync = open.sync.with_server_scheduling(true);
            tracing::debug!(
                account_id = sync.account_id().as_str(),
                calendar_id = sync.calendar_id().as_str(),
                writable = open.writable,
                "calendar backend connected"
            );
            // Push starts after connection is stored so refreshes find a live session.
            backend.install_connection(sync, |sync| start_push(meta_backend, sync));
            // Sets writable on the backend; EDS overwrites connected-writable from this value.
            e_cal_backend_set_writable(
                meta_backend.cast(),
                if open.writable { GTRUE } else { GFALSE },
            );
            GTRUE
        })
    }
}

unsafe extern "C" fn disconnect_sync(
    meta_backend: *mut ECalMetaBackend,
    _cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Follows safety requirements of connect_sync.
    unsafe {
        guard_bool("disconnect_sync", error, || {
            // No-op if already disconnected.
            if let Some(backend) = instance(meta_backend) {
                // Stop push before dropping connection credentials.
                let unsubscribed = backend.stop_push();
                let dropped = backend.drop_connection();
                tracing::debug!(dropped, unsubscribed, "calendar backend disconnected");
            }
            GTRUE
        })
    }
}

unsafe extern "C" fn list_existing_sync(
    meta_backend: *mut ECalMetaBackend,
    out_new_sync_tag: *mut *mut gchar,
    out_existing_objects: *mut *mut GSList,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Follows safety requirements of connect_sync.
    unsafe {
        with_connection(
            "list_existing_sync",
            meta_backend,
            cancellable,
            error,
            |sync, error| {
                let listed =
                    ops::list_existing(sync, out_new_sync_tag, out_existing_objects, error);
                if listed != GFALSE && !out_existing_objects.is_null() {
                    keep_cached_versions(meta_backend, *out_existing_objects, cancellable);
                }
                listed
            },
        )
    }
}

#[allow(clippy::too_many_arguments)] // C vfunc signature requires all parameters.
unsafe extern "C" fn get_changes_sync(
    meta_backend: *mut ECalMetaBackend,
    last_sync_tag: *const gchar,
    is_repeat: gboolean,
    out_new_sync_tag: *mut *mut gchar,
    out_repeat: *mut gboolean,
    out_created_objects: *mut *mut GSList,
    out_modified_objects: *mut *mut GSList,
    out_removed_objects: *mut *mut GSList,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Follows safety requirements of connect_sync.
    unsafe {
        with_connection(
            "get_changes_sync",
            meta_backend,
            cancellable,
            error,
            |sync, error| {
                match ops::get_changes(
                    sync,
                    last_sync_tag,
                    is_repeat,
                    out_new_sync_tag,
                    out_repeat,
                    out_created_objects,
                    out_modified_objects,
                    out_removed_objects,
                    error,
                ) {
                    Outcome::Reported => {
                        if !out_modified_objects.is_null() {
                            keep_cached_versions(meta_backend, *out_modified_objects, cancellable);
                        }
                        GTRUE
                    }
                    Outcome::Failed => GFALSE,
                    // Chain up to parent list implementation when delta sync cannot be calculated.
                    Outcome::ListInstead => {
                        match parent_class().and_then(|class| class.get_changes_sync) {
                            Some(chain_up) => chain_up(
                                meta_backend,
                                last_sync_tag,
                                is_repeat,
                                out_new_sync_tag,
                                out_repeat,
                                out_created_objects,
                                out_modified_objects,
                                out_removed_objects,
                                cancellable,
                                error,
                            ),
                            // Fail offline if parent vfunc is missing.
                            None => fail_offline(error),
                        }
                    }
                }
            },
        )
    }
}

unsafe extern "C" fn load_component_sync(
    meta_backend: *mut ECalMetaBackend,
    uid: *const gchar,
    extra: *const gchar,
    out_component: *mut *mut ICalComponent,
    out_extra: *mut *mut gchar,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Follows safety requirements of connect_sync.
    unsafe {
        with_connection(
            "load_component_sync",
            meta_backend,
            cancellable,
            error,
            |sync, error| ops::load_component(sync, uid, extra, out_component, out_extra, error),
        )
    }
}

#[allow(clippy::too_many_arguments)] // C vfunc signature requires all parameters.
unsafe extern "C" fn save_component_sync(
    meta_backend: *mut ECalMetaBackend,
    overwrite_existing: gboolean,
    conflict_resolution: EConflictResolution,
    instances: *const GSList,
    extra: *const gchar,
    // Operation flags indicate whether user suppressed iTIP dispatch.
    opflags: ECalOperationFlags,
    out_new_uid: *mut *mut gchar,
    out_new_extra: *mut *mut gchar,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Follows safety requirements of connect_sync, with instances pointing to valid ECalComponent list.
    unsafe {
        with_connection(
            "save_component_sync",
            meta_backend,
            cancellable,
            error,
            |sync, error| {
                // ECalBackend implements ETimezoneCache, resolving timezones for appointments.
                ops::save_component(
                    sync,
                    overwrite_existing,
                    conflict_resolution,
                    instances,
                    extra,
                    meta_backend.cast(),
                    scheduling::itip(opflags),
                    out_new_uid,
                    out_new_extra,
                    error,
                )
            },
        )
    }
}

#[allow(clippy::too_many_arguments)] // C vfunc signature requires all parameters.
unsafe extern "C" fn remove_component_sync(
    meta_backend: *mut ECalMetaBackend,
    _conflict_resolution: EConflictResolution,
    uid: *const gchar,
    _extra: *const gchar,
    _object: *const gchar,
    opflags: ECalOperationFlags,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Follows safety requirements of connect_sync.
    unsafe {
        with_connection(
            "remove_component_sync",
            meta_backend,
            cancellable,
            error,
            |sync, error| ops::remove_component(sync, uid, scheduling::itip(opflags), error),
        )
    }
}

/// Pushes local color changes from ESource modifications.
///
/// # Safety
///
/// `meta_backend` must be null or point to a valid instance with configured ESource.
unsafe extern "C" fn source_changed(meta_backend: *mut ECalMetaBackend) {
    // SAFETY: Follows safety requirements of connect_sync.
    unsafe {
        guard("source_changed", (), || {
            let Some(backend) = instance(meta_backend) else {
                return;
            };
            let Some(session) = backend.session() else {
                return;
            };
            let Some(baseline_slot) = backend.last_known_color() else {
                return;
            };

            let guard = read(session);
            let Some(sync) = guard.as_ref() else {
                return;
            };

            let source = e_backend_get_source(meta_backend.cast());
            // SAFETY: `source` belongs to a valid backend instance.
            let current = marshal::selectable_color(source);

            let mut baseline = write(baseline_slot);
            match ops::on_source_changed(sync, current.as_deref(), baseline.as_deref()) {
                ops::ColorOutcome::Unchanged | ops::ColorOutcome::Failed => {}
                ops::ColorOutcome::Pushed(pushed) => *baseline = pushed,
            }
        });
    }
}

/// Queries free/busy intervals, falling back to parent cache computation if disconnected.
#[allow(clippy::too_many_arguments)] // C vfunc signature requires all parameters.
unsafe extern "C" fn get_free_busy_sync(
    backend: *mut ECalBackendSync,
    cal: *mut EDataCal,
    cancellable: *mut GCancellable,
    users: *const GSList,
    start: time_t,
    end: time_t,
    out_freebusy: *mut *mut GSList,
    error: *mut *mut GError,
) {
    // SAFETY: Follows safety requirements of connect_sync.
    unsafe {
        guard_value("get_free_busy_sync", error, (), || {
            let _cancel = observe(cancellable);
            // Drop read lock before chaining up to parent cache search.
            let outcome = {
                let session = instance(backend.cast()).and_then(JmapCalBackend::session);
                let guard = session.map(read);
                match guard.as_deref().and_then(Option::as_ref) {
                    Some(sync) => ops::get_free_busy(sync, users, start, end, out_freebusy, error),
                    None => ops::FreeBusyOutcome::NothingKnown,
                }
            };

            if matches!(outcome, ops::FreeBusyOutcome::NothingKnown)
                && let Some(chain_up) =
                    parent_class().and_then(|class| class.parent_class.get_free_busy_sync)
            {
                // Clear error state before chaining up to parent.
                chain_up(
                    backend,
                    cal,
                    cancellable,
                    users,
                    start,
                    end,
                    out_freebusy,
                    error,
                );
            }
        })
    }
}

/// Dismisses one reminder by setting alarm acknowledged timestamp in cached component.
///
/// Saves the modified component through meta backend, sending acknowledged state to server (RFC 8984 §4.5.2).
/// The save never asks the server to mail the guests, whatever `opflags` says.
#[allow(clippy::too_many_arguments)] // C vfunc signature requires all parameters.
unsafe extern "C" fn discard_alarm_sync(
    backend: *mut ECalBackendSync,
    cal: *mut EDataCal,
    cancellable: *mut GCancellable,
    uid: *const gchar,
    rid: *const gchar,
    auid: *const gchar,
    opflags: ECalOperationFlags,
    error: *mut *mut GError,
) {
    // SAFETY: Follows safety requirements of connect_sync.
    unsafe {
        guard_value("discard_alarm_sync", error, (), || {
            let Some((component, occurrence)) =
                cached_component(backend.cast(), uid, rid, cancellable)
            else {
                return fail_not_found(error);
            };
            if auid.is_null() || e_cal_util_set_alarm_acknowledged(component, auid, 0) == GFALSE {
                g_object_unref(component.cast());
                return fail_not_found(error);
            }

            let objects = g_slist_prepend(
                ptr::null_mut(),
                e_cal_component_get_as_string(component).cast(),
            );
            let scope = if occurrence {
                E_CAL_OBJ_MOD_THIS
            } else {
                E_CAL_OBJ_MOD_ALL
            };
            let mut old_components: *mut GSList = ptr::null_mut();
            let mut new_components: *mut GSList = ptr::null_mut();
            e_cal_backend_sync_modify_objects(
                backend,
                cal,
                cancellable,
                objects,
                scope,
                scheduling::dismissal(opflags),
                &mut old_components,
                &mut new_components,
                error,
            );
            e_util_free_nullable_object_slist(old_components);
            e_util_free_nullable_object_slist(new_components);
            g_slist_free_full(objects, Some(g_free));
            g_object_unref(component.cast());
            tracing::debug!(occurrence, "dismissed a reminder");
        })
    }
}

/// Chains up to the meta backend with the opflags marked as received.
///
/// Evolution calls this for an accepted invitation and for an import. The data
/// comes from another person, so the sync layer applies [`Itip::Received`].
/// The call fails while the backend cannot connect. If the backend goes offline
/// after this check, the meta backend queues the change with the marked opflags.
unsafe extern "C" fn receive_objects_sync(
    backend: *mut ECalBackendSync,
    cal: *mut EDataCal,
    cancellable: *mut GCancellable,
    calobj: *const gchar,
    opflags: ECalOperationFlags,
    error: *mut *mut GError,
) {
    // SAFETY: Follows safety requirements of connect_sync.
    unsafe {
        guard_value("receive_objects_sync", error, (), || {
            let Some(parent) =
                parent_class().and_then(|class| class.parent_class.receive_objects_sync)
            else {
                return;
            };
            if e_cal_meta_backend_ensure_connected_sync(backend.cast(), cancellable, error)
                == GFALSE
            {
                return;
            }
            let opflags = scheduling::received(opflags);
            parent(backend, cal, cancellable, calobj, opflags, error);
        })
    }
}

/// Returns the cached component at `rid`, or the master instance if unexploded.
///
/// # Safety
///
/// `backend` must point to a valid instance; strings and cancellable must be valid or null.
unsafe fn cached_component(
    backend: *mut ECalMetaBackend,
    uid: *const gchar,
    rid: *const gchar,
    cancellable: *mut GCancellable,
) -> Option<(*mut ECalComponent, bool)> {
    if uid.is_null() {
        return None;
    }
    // SAFETY: `backend` is a valid instance. Returned cache is a new reference or null.
    let cache = unsafe { e_cal_meta_backend_ref_cache(backend) };
    if cache.is_null() {
        return None;
    }
    // SAFETY: `rid` is null or a valid NUL-terminated string.
    let occurrence = !rid.is_null() && unsafe { *rid } != 0;
    let mut component: *mut ECalComponent = ptr::null_mut();
    let lookup = |rid: *const gchar, out: &mut *mut ECalComponent| {
        // SAFETY: Lookups query the valid cache reference with valid strings.
        unsafe {
            e_cal_cache_get_component(cache, uid, rid, out, cancellable, ptr::null_mut()) != GFALSE
        }
    };
    let found = if lookup(rid, &mut component) {
        Some((component, occurrence))
    } else if occurrence && lookup(ptr::null(), &mut component) {
        Some((component, false))
    } else {
        None
    };
    // SAFETY: Drops the referenced cache object.
    unsafe { g_object_unref(cache.cast()) };
    found
}

/// Adds the server versions in the cached extras to the infos of `list`.
///
/// # Safety
///
/// `backend` must point to a valid instance, `list` must be null or a list that
/// `marshal::info_list` made, and `cancellable` must be valid or null.
unsafe fn keep_cached_versions(
    backend: *mut ECalMetaBackend,
    list: *mut GSList,
    cancellable: *mut GCancellable,
) {
    if list.is_null() {
        return;
    }
    // SAFETY: `backend` is a valid instance. Returned cache is a new reference or null.
    let cache = unsafe { e_cal_meta_backend_ref_cache(backend) };
    if cache.is_null() {
        return;
    }
    let cached = |uid: *const gchar| {
        let mut extra: *mut gchar = ptr::null_mut();
        // SAFETY: the cache reference and `uid` are valid; the call gives an owned string.
        unsafe {
            if e_cal_cache_get_component_extra(
                cache,
                uid,
                ptr::null(),
                &mut extra,
                cancellable,
                ptr::null_mut(),
            ) == GFALSE
            {
                g_free(extra.cast());
                return None;
            }
            let text = read_string(extra);
            g_free(extra.cast());
            text
        }
    };
    // SAFETY: the caller guarantees the list shape.
    unsafe { marshal::keep_versions(list, cached) };
    // SAFETY: Drops the referenced cache object.
    unsafe { g_object_unref(cache.cast()) };
}

/// Sets `E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND` on `error`.
///
/// # Safety
///
/// Follows safety requirements of [`set_raw_gerror`].
unsafe fn fail_not_found(error: *mut *mut GError) {
    // SAFETY: Error code is valid and null message requests default translated text.
    unsafe {
        set_raw_gerror(
            error,
            e_cal_client_error_create(E_CAL_CLIENT_ERROR_OBJECT_NOT_FOUND, ptr::null()),
        );
    }
}

/// Returns backend capabilities with the server scheduling capabilities added.
unsafe extern "C" fn get_backend_property(
    backend: *mut ECalBackend,
    prop_name: *const gchar,
) -> *mut gchar {
    // SAFETY: `backend` is a valid instance and `prop_name` is null or NUL-terminated.
    unsafe {
        guard("get_backend_property", ptr::null_mut(), || {
            let inherited = match parent_class()
                .and_then(|class| class.parent_class.parent_class.impl_get_backend_property)
            {
                Some(chain_up) => chain_up(backend, prop_name),
                None => ptr::null_mut(),
            };
            if prop_name.is_null() || CStr::from_ptr(prop_name) != CAPABILITIES_PROPERTY {
                return inherited;
            }

            let caps = scheduling::capabilities(read_string(inherited).as_deref());
            g_free(inherited.cast());
            dup_string(&caps)
        })
    }
}

// ---------------------------------------------------------------------------
// the shared shape

/// Runs `f` against live connection under panic guard, cancellation, and OAuth retry.
///
/// # Safety
///
/// `meta_backend` must be null or point to a valid instance.
unsafe fn with_connection(
    context: &str,
    meta_backend: *mut ECalMetaBackend,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
    mut f: impl FnMut(&CalSync, *mut *mut GError) -> gboolean,
) -> gboolean {
    unsafe {
        guard_bool(context, error, || {
            let _cancel = observe(cancellable);
            let Some(session) = instance(meta_backend).and_then(JmapCalBackend::session) else {
                return fail_offline(error);
            };
            // Hold read lock across retries; token refresh updates credentials in place.
            let guard = read(session);
            let Some(sync) = guard.as_ref() else {
                return fail_offline(error);
            };
            retry_on_authentication_failure(
                error,
                |slot| f(sync, slot),
                || refresh_credentials(meta_backend, sync, cancellable),
            )
        })
    }
}

/// JMAP data types subscribed for push notifications.
const PUSHED_TYPES: &[&str] = &["CalendarEvent", "Calendar"];

/// Subscribes to JMAP push notifications if supported by server.
///
/// # Safety
///
/// `meta_backend` must point to a valid GObject-derived instance.
unsafe fn start_push(meta_backend: *mut ECalMetaBackend, sync: &CalSync) -> Option<PushRefresh> {
    // SAFETY: `meta_backend` points to a live GObject instance during vfunc execution.
    unsafe {
        push::start_for(
            meta_backend.cast(),
            sync.client(),
            sync.account_id(),
            PUSHED_TYPES,
            schedule_refresh,
        )
    }
}

/// Schedules a refresh in response to push notification.
///
/// # Safety
///
/// `object` must point to a live `ECalMetaBackend`.
unsafe extern "C" fn schedule_refresh(object: *mut gobject_sys::GObject) {
    // SAFETY: `object` points to a live ECalMetaBackend per function contract.
    unsafe { e_cal_meta_backend_schedule_refresh(object.cast()) };
}

/// Refreshes OAuth 2.0 access token and updates active connection credentials.
///
/// # Safety
///
/// `meta_backend` must point to a valid instance and `cancellable` be valid or null.
unsafe fn refresh_credentials(
    meta_backend: *mut ECalMetaBackend,
    sync: &CalSync,
    cancellable: *mut GCancellable,
) -> bool {
    // SAFETY: `meta_backend` is a valid EBackend instance.
    let source = unsafe { backend_source(meta_backend.cast()) };
    // SAFETY: `source` is null or a valid ESource pointer.
    if source.is_null() || !unsafe { source_uses_oauth2(source) } {
        return false;
    }

    // SAFETY: `source` is checked non-null and cancellable matches caller contract.
    match unsafe { access_token(source, cancellable) } {
        Ok(token) => {
            tracing::debug!("refreshed the calendar connection's OAuth 2.0 access token");
            sync.client().set_credentials(Credentials::bearer(token));
            if let Some(header) = sync.client().authorization_header()
                // SAFETY: Forwards preconditions from caller.
                && let Some(backend) = unsafe { instance(meta_backend) }
            {
                backend.refresh_push_headers(vec![("Authorization".to_owned(), header)]);
            }
            true
        }
        Err(failure) => {
            // Logged at debug; caller will surface authentication error to EDS.
            tracing::debug!(
                ?failure,
                "could not refresh the calendar connection's OAuth 2.0 access token"
            );
            false
        }
    }
}

/// Converts EDS backend pointer to a Rust reference.
///
/// # Safety
///
/// `meta_backend` must be null or point to a valid instance of this type.
unsafe fn instance<'a>(meta_backend: *mut ECalMetaBackend) -> Option<&'a JmapCalBackend> {
    unsafe { meta_backend.cast::<JmapCalBackend>().as_ref() }
}

/// Sets `E_CLIENT_ERROR_REPOSITORY_OFFLINE` on `error`.
///
/// # Safety
///
/// Follows safety requirements of [`set_raw_gerror`].
unsafe fn fail_offline(error: *mut *mut GError) -> gboolean {
    let message = cstring_lossy("the JMAP calendar is not connected");
    // SAFETY: Error code is valid and message string is copied.
    unsafe {
        set_raw_gerror(
            error,
            e_client_error_create(E_CLIENT_ERROR_REPOSITORY_OFFLINE, message.as_ptr()),
        );
    }
    GFALSE
}

/// Returns the destination host and port for reachability monitoring.
unsafe extern "C" fn get_destination_address(
    backend: *mut EBackend,
    host: *mut *mut gchar,
    port: *mut guint16,
) -> gboolean {
    guard("get_destination_address", GFALSE, || {
        // SAFETY: EDS passes a valid backend pointer for the duration of the call.
        let source = unsafe { e_backend_get_source(backend) };
        if source.is_null() {
            return GFALSE;
        }

        // SAFETY: `source` is a valid account source pointer.
        let Some((address_host, address_port)) = (unsafe { destination_address(source) }) else {
            return GFALSE;
        };

        tracing::debug!(
            host = address_host.as_str(),
            port = address_port,
            "resolved destination address for calendar backend"
        );

        // SAFETY: `host` and `port` are valid out-parameters written on success path.
        unsafe {
            *host = dup_string(&address_host);
            *port = address_port;
        }
        GTRUE
    })
}
