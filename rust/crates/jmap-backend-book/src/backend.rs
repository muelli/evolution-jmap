// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `EBookMetaBackend` subclass implementing EDS address book vfuncs.
//!
//! Bridges instance and class structs to [`ops`], managing connection and push lifecycles.

use std::ffi::CStr;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::Duration;

use eds_sys::{
    E_CLIENT_ERROR_REPOSITORY_OFFLINE, E_CONTACT_ADDRESS, E_CONTACT_ADDRESS_HOME,
    E_CONTACT_ADDRESS_LABEL_HOME, E_CONTACT_ADDRESS_LABEL_OTHER, E_CONTACT_ADDRESS_LABEL_WORK,
    E_CONTACT_ADDRESS_OTHER, E_CONTACT_ADDRESS_WORK, E_CONTACT_ANNIVERSARY, E_CONTACT_ASSISTANT,
    E_CONTACT_BIRTH_DATE, E_CONTACT_BLOG_URL, E_CONTACT_CALENDAR_URI, E_CONTACT_CATEGORIES,
    E_CONTACT_CATEGORY_LIST, E_CONTACT_EMAIL, E_CONTACT_EMAIL_1, E_CONTACT_EMAIL_2,
    E_CONTACT_EMAIL_3, E_CONTACT_EMAIL_4, E_CONTACT_FAMILY_NAME, E_CONTACT_FILE_AS,
    E_CONTACT_FREEBUSY_URL, E_CONTACT_FULL_NAME, E_CONTACT_GIVEN_NAME, E_CONTACT_HOMEPAGE_URL,
    E_CONTACT_IM_AIM, E_CONTACT_IM_AIM_HOME_1, E_CONTACT_IM_AIM_HOME_2, E_CONTACT_IM_AIM_HOME_3,
    E_CONTACT_IM_AIM_WORK_1, E_CONTACT_IM_AIM_WORK_2, E_CONTACT_IM_AIM_WORK_3,
    E_CONTACT_IM_GADUGADU, E_CONTACT_IM_GADUGADU_HOME_1, E_CONTACT_IM_GADUGADU_HOME_2,
    E_CONTACT_IM_GADUGADU_HOME_3, E_CONTACT_IM_GADUGADU_WORK_1, E_CONTACT_IM_GADUGADU_WORK_2,
    E_CONTACT_IM_GADUGADU_WORK_3, E_CONTACT_IM_GOOGLE_TALK, E_CONTACT_IM_GOOGLE_TALK_HOME_1,
    E_CONTACT_IM_GOOGLE_TALK_HOME_2, E_CONTACT_IM_GOOGLE_TALK_HOME_3,
    E_CONTACT_IM_GOOGLE_TALK_WORK_1, E_CONTACT_IM_GOOGLE_TALK_WORK_2,
    E_CONTACT_IM_GOOGLE_TALK_WORK_3, E_CONTACT_IM_GROUPWISE, E_CONTACT_IM_GROUPWISE_HOME_1,
    E_CONTACT_IM_GROUPWISE_HOME_2, E_CONTACT_IM_GROUPWISE_HOME_3, E_CONTACT_IM_GROUPWISE_WORK_1,
    E_CONTACT_IM_GROUPWISE_WORK_2, E_CONTACT_IM_GROUPWISE_WORK_3, E_CONTACT_IM_ICQ,
    E_CONTACT_IM_ICQ_HOME_1, E_CONTACT_IM_ICQ_HOME_2, E_CONTACT_IM_ICQ_HOME_3,
    E_CONTACT_IM_ICQ_WORK_1, E_CONTACT_IM_ICQ_WORK_2, E_CONTACT_IM_ICQ_WORK_3, E_CONTACT_IM_JABBER,
    E_CONTACT_IM_JABBER_HOME_1, E_CONTACT_IM_JABBER_HOME_2, E_CONTACT_IM_JABBER_HOME_3,
    E_CONTACT_IM_JABBER_WORK_1, E_CONTACT_IM_JABBER_WORK_2, E_CONTACT_IM_JABBER_WORK_3,
    E_CONTACT_IM_MATRIX, E_CONTACT_IM_MATRIX_HOME_1, E_CONTACT_IM_MATRIX_HOME_2,
    E_CONTACT_IM_MATRIX_HOME_3, E_CONTACT_IM_MATRIX_WORK_1, E_CONTACT_IM_MATRIX_WORK_2,
    E_CONTACT_IM_MATRIX_WORK_3, E_CONTACT_IM_MSN, E_CONTACT_IM_MSN_HOME_1, E_CONTACT_IM_MSN_HOME_2,
    E_CONTACT_IM_MSN_HOME_3, E_CONTACT_IM_MSN_WORK_1, E_CONTACT_IM_MSN_WORK_2,
    E_CONTACT_IM_MSN_WORK_3, E_CONTACT_IM_SKYPE, E_CONTACT_IM_SKYPE_HOME_1,
    E_CONTACT_IM_SKYPE_HOME_2, E_CONTACT_IM_SKYPE_HOME_3, E_CONTACT_IM_SKYPE_WORK_1,
    E_CONTACT_IM_SKYPE_WORK_2, E_CONTACT_IM_SKYPE_WORK_3, E_CONTACT_IM_YAHOO,
    E_CONTACT_IM_YAHOO_HOME_1, E_CONTACT_IM_YAHOO_HOME_2, E_CONTACT_IM_YAHOO_HOME_3,
    E_CONTACT_IM_YAHOO_WORK_1, E_CONTACT_IM_YAHOO_WORK_2, E_CONTACT_IM_YAHOO_WORK_3,
    E_CONTACT_MANAGER, E_CONTACT_NAME, E_CONTACT_NICKNAME, E_CONTACT_NOTE, E_CONTACT_OFFICE,
    E_CONTACT_ORG, E_CONTACT_ORG_UNIT, E_CONTACT_PHONE_ASSISTANT, E_CONTACT_PHONE_BUSINESS,
    E_CONTACT_PHONE_BUSINESS_2, E_CONTACT_PHONE_BUSINESS_FAX, E_CONTACT_PHONE_CALLBACK,
    E_CONTACT_PHONE_CAR, E_CONTACT_PHONE_COMPANY, E_CONTACT_PHONE_HOME, E_CONTACT_PHONE_HOME_2,
    E_CONTACT_PHONE_HOME_FAX, E_CONTACT_PHONE_ISDN, E_CONTACT_PHONE_MOBILE, E_CONTACT_PHONE_OTHER,
    E_CONTACT_PHONE_OTHER_FAX, E_CONTACT_PHONE_PAGER, E_CONTACT_PHONE_PRIMARY,
    E_CONTACT_PHONE_RADIO, E_CONTACT_PHONE_TELEX, E_CONTACT_PHONE_TTYTDD, E_CONTACT_PHOTO,
    E_CONTACT_ROLE, E_CONTACT_SPOUSE, E_CONTACT_TEL, E_CONTACT_TITLE, E_CONTACT_UID,
    E_CONTACT_VIDEO_URL, EBackend, EBookBackend, EBookMetaBackend, EBookMetaBackendClass,
    EConflictResolution, EContact, EContactField, ENamedParameters, ESourceAuthenticationResult,
    GTlsCertificateFlags, e_backend_get_source, e_book_backend_schedule_custom_operation,
    e_book_backend_set_writable, e_book_cache_get_contact_extra, e_book_meta_backend_get_type,
    e_book_meta_backend_ref_cache, e_book_meta_backend_refresh_sync, e_client_error_create,
    e_contact_field_name,
};
use gio_sys::GCancellable;
use glib_sys::{
    GError, GFALSE, GSList, GTRUE, GType, g_clear_error, g_free, gboolean, gchar, gpointer,
    guint16, guint32,
};
use gobject_sys::g_object_unref;
use jmap_backend_core::cancel::observe;
use jmap_backend_core::error::{cstring_lossy, set_raw_gerror};
use jmap_backend_core::instance::Slot;
#[cfg(feature = "testing")]
use jmap_backend_core::instance::zeroed_box;
use jmap_backend_core::invoke::wait_for_queued;
use jmap_backend_core::marshal::{dup_string, read_string};
use jmap_backend_core::oauth2::{access_token, source_uses_oauth2};
use jmap_backend_core::poison::{lock, read, write};
use jmap_backend_core::push::{self, PushRefresh};
use jmap_backend_core::retry::retry_on_authentication_failure;
use jmap_backend_core::source::{backend_source, destination_address};
use jmap_backend_core::subclass::{self, ObjectSubclass};
use jmap_backend_core::trampoline::{guard, guard_bool};
use jmap_book_sync::BookSync;
use jmap_client::Credentials;

use crate::connect::{self, ACCEPTED_AUTH_RESULT, write_auth_result};
use crate::marshal;
use crate::ops::{self, Outcome};

/// The JMAP address book backend type.
#[repr(C)]
pub struct JmapBookBackend {
    /// Parent GObject instance.
    parent: EBookMetaBackend,
    /// Live connection; read-locked across concurrent vfuncs, write-locked on connect/disconnect.
    session: Slot<RwLock<Option<BookSync>>>,
    /// Active push subscription for server-driven change notifications.
    push: Slot<Mutex<Option<PushRefresh>>>,
    /// Whether a push refresh is in the queue and has not started.
    refresh_queued: AtomicBool,
}

/// Class struct for [`JmapBookBackend`].
#[repr(C)]
pub struct JmapBookBackendClass {
    pub parent_class: EBookMetaBackendClass,
}

impl JmapBookBackend {
    /// Installs `sync` as the live connection, replacing any existing connection.
    pub fn store_connection(&self, sync: BookSync) {
        if let Some(session) = self.session() {
            *write(session) = Some(sync);
        }
    }

    /// Installs `sync` as the live connection and starts the push subscription.
    pub fn install_connection(
        &self,
        sync: BookSync,
        start_push: impl FnOnce(&BookSync) -> Option<PushRefresh>,
    ) {
        self.store_connection(sync);
        let push = self.session().and_then(|session| {
            let guard = read(session);
            guard.as_ref().and_then(start_push)
        });
        if let Some(push) = push {
            self.store_push(push);
        }
    }

    /// Drops the connection, reporting whether there was one.
    pub fn drop_connection(&self) -> bool {
        match self.session() {
            Some(session) => write(session).take().is_some(),
            None => false,
        }
    }

    /// Installs `push` as the live push subscription, stopping and dropping
    /// any prior subscription.
    pub fn store_push(&self, push: PushRefresh) {
        if let Some(slot) = self.push.get() {
            *lock(slot) = Some(push);
        }
    }

    /// Stops the push subscription and waits for its thread, reporting
    /// whether there was one. Once this returns, no refresh can still be
    /// scheduled from it.
    pub fn stop_push(&self) -> bool {
        match self.push.get() {
            Some(slot) => lock(slot).take().is_some(),
            None => false,
        }
    }

    /// Whether a push subscription is live.
    #[cfg(feature = "testing")]
    pub fn is_pushing(&self) -> bool {
        self.push.get().is_some_and(|slot| lock(slot).is_some())
    }

    /// Replaces the authorization headers the live push subscription sends
    /// on future reconnect attempts, if a subscription is active.
    pub fn refresh_push_headers(&self, headers: Vec<(String, String)>) {
        if let Some(slot) = self.push.get()
            && let Some(push) = lock(slot).as_ref()
        {
            push.set_headers(headers);
        }
    }

    /// Whether an operation would find a connection.
    pub fn is_connected(&self) -> bool {
        self.session()
            .is_some_and(|session| read(session).is_some())
    }

    /// Detached instance for tests without requiring a running D-Bus registry daemon.
    #[cfg(feature = "testing")]
    pub fn detached() -> Box<Self> {
        // SAFETY: Parent fields are pointers or integers with valid all-zero states.
        // An all-zero `Slot` represents the documented empty state.
        let backend: Box<Self> = unsafe { zeroed_box() };
        backend.session.init(RwLock::new(None));
        backend.push.init(Mutex::new(None));
        backend
    }

    /// Inspects the live connection under a read lock; for testing only.
    #[cfg(feature = "testing")]
    pub fn inspect_connection<R>(&self, f: impl FnOnce(&BookSync) -> R) -> Option<R> {
        let session = self.session()?;
        let guard = read(session);
        guard.as_ref().map(f)
    }

    /// The connection slot, or `None` on an instance whose `instance_init` has
    /// not run or whose `finalize` already has.
    fn session(&self) -> Option<&RwLock<Option<BookSync>>> {
        self.session.get()
    }
}

// SAFETY: Structs are #[repr(C)] and lead with EBookMetaBackend structs. EBookMetaBackend derives from GObject.
unsafe impl ObjectSubclass for JmapBookBackend {
    const NAME: &'static CStr = c"EBookBackendJmap";
    type Instance = JmapBookBackend;
    type Class = JmapBookBackendClass;

    fn parent_type() -> GType {
        // SAFETY: No arguments; EDS type system initialises itself.
        unsafe { e_book_meta_backend_get_type() }
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` points to a freshly allocated class struct; slots are in the parent class struct.
        let vfuncs = unsafe { &mut (*class).parent_class };
        vfuncs.connect_sync = Some(connect_sync);
        vfuncs.disconnect_sync = Some(disconnect_sync);
        vfuncs.list_existing_sync = Some(list_existing_sync);
        vfuncs.get_changes_sync = Some(get_changes_sync);
        vfuncs.load_contact_sync = Some(load_contact_sync);
        vfuncs.save_contact_sync = Some(save_contact_sync);
        vfuncs.remove_contact_sync = Some(remove_contact_sync);
        // Override EBookBackendClass::impl_get_backend_property for supported-fields and required-fields.
        vfuncs.parent_class.parent_class.impl_get_backend_property = Some(get_backend_property);
        // Override EBackendClass::get_destination_address using account authentication host.
        vfuncs
            .parent_class
            .parent_class
            .parent_class
            .get_destination_address = Some(get_destination_address);
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // SAFETY: `instance` points to a zeroed instance struct, and a zeroed Slot is empty.
        unsafe { (*instance).session.init(RwLock::new(None)) };
        // SAFETY: as above.
        unsafe { (*instance).push.init(Mutex::new(None)) };
    }

    unsafe fn finalize(instance: *mut Self::Instance) {
        // Stop push thread first before clearing the session to avoid callbacks during finalize.
        // SAFETY: Instance is finalizing; no borrows handed out by get remain alive.
        unsafe { (*instance).push.clear() };
        // SAFETY: Drops the connection and closes the socket during finalization.
        unsafe { (*instance).session.clear() };
    }
}

/// Returns the parent `EBookMetaBackendClass` for chaining up vfuncs.
pub fn parent_class() -> Option<&'static EBookMetaBackendClass> {
    // SAFETY: Forwarded to parent_class helper.
    unsafe {
        subclass::parent_class::<EBookMetaBackendClass>(
            <JmapBookBackend as ObjectSubclass>::parent_type(),
        )
    }
}

// ---------------------------------------------------------------------------
// the vfunc slots

unsafe extern "C" fn connect_sync(
    meta_backend: *mut EBookMetaBackend,
    credentials: *const ENamedParameters,
    out_auth_result: *mut ESourceAuthenticationResult,
    // Certificate out-parameters are unused; transport errors report TLS failures directly.
    _out_certificate_pem: *mut *mut gchar,
    _out_certificate_errors: *mut GTlsCertificateFlags,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: Valid instance pointer, valid or NULL parameters, and writable out-parameters.
    unsafe {
        guard_bool("connect_sync", error, || {
            let Some(backend) = instance(meta_backend) else {
                return fail_offline(error);
            };

            // EDS calls this whenever it suspects the connection is gone,
            // including when it is not. Re-opening a live one would drop a
            // socket other threads are mid-request on.
            if backend.is_connected() {
                tracing::debug!("reusing existing address book connection");
                write_auth_result(out_auth_result, ACCEPTED_AUTH_RESULT);
                return GTRUE;
            }

            let source = e_backend_get_source(meta_backend.cast());
            let Some(open) =
                connect::connect(source, credentials, cancellable, out_auth_result, error)
            else {
                return GFALSE;
            };

            tracing::debug!(
                account_id = open.sync.account_id().as_str(),
                address_book_id = open.sync.address_book_id().as_str(),
                writable = open.writable,
                "address book backend connected"
            );
            // Connection must be stored before starting push to avoid race with incoming refresh.
            backend.install_connection(open.sync, |sync| start_push(meta_backend, sync));
            // Mark backend writable on EBookBackend according to its server rights.
            e_book_backend_set_writable(
                meta_backend.cast(),
                if open.writable { GTRUE } else { GFALSE },
            );
            GTRUE
        })
    }
}

unsafe extern "C" fn disconnect_sync(
    meta_backend: *mut EBookMetaBackend,
    _cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`.
    unsafe {
        guard_bool("disconnect_sync", error, || {
            // Clean up push subscription first, then drop connection.
            if let Some(backend) = instance(meta_backend) {
                let unsubscribed = backend.stop_push();
                let dropped = backend.drop_connection();
                tracing::debug!(dropped, unsubscribed, "address book backend disconnected");
            }
            GTRUE
        })
    }
}

unsafe extern "C" fn list_existing_sync(
    meta_backend: *mut EBookMetaBackend,
    out_new_sync_tag: *mut *mut gchar,
    out_existing_objects: *mut *mut GSList,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`.
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

#[allow(clippy::too_many_arguments)] // Matches the EDS vfunc signature.
unsafe extern "C" fn get_changes_sync(
    meta_backend: *mut EBookMetaBackend,
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
    // SAFETY: as `connect_sync`.
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
                    // Fall back to parent EBookMetaBackend implementation when changes cannot be calculated.
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
                            None => fail_offline(error),
                        }
                    }
                }
            },
        )
    }
}

unsafe extern "C" fn load_contact_sync(
    meta_backend: *mut EBookMetaBackend,
    uid: *const gchar,
    extra: *const gchar,
    out_contact: *mut *mut EContact,
    out_extra: *mut *mut gchar,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`.
    unsafe {
        with_connection(
            "load_contact_sync",
            meta_backend,
            cancellable,
            error,
            |sync, error| ops::load_contact(sync, uid, extra, out_contact, out_extra, error),
        )
    }
}

#[allow(clippy::too_many_arguments)] // Matches the EDS vfunc signature.
unsafe extern "C" fn save_contact_sync(
    meta_backend: *mut EBookMetaBackend,
    overwrite_existing: gboolean,
    conflict_resolution: EConflictResolution,
    contact: *mut EContact,
    extra: *const gchar,
    _opflags: guint32,
    out_new_uid: *mut *mut gchar,
    out_new_extra: *mut *mut gchar,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`.
    unsafe {
        with_connection(
            "save_contact_sync",
            meta_backend,
            cancellable,
            error,
            |sync, error| {
                ops::save_contact(
                    sync,
                    overwrite_existing,
                    conflict_resolution,
                    contact,
                    extra,
                    out_new_uid,
                    out_new_extra,
                    error,
                )
            },
        )
    }
}

#[allow(clippy::too_many_arguments)] // Matches the EDS vfunc signature.
unsafe extern "C" fn remove_contact_sync(
    meta_backend: *mut EBookMetaBackend,
    _conflict_resolution: EConflictResolution,
    uid: *const gchar,
    _extra: *const gchar,
    _object: *const gchar,
    _opflags: guint32,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
) -> gboolean {
    // SAFETY: as `connect_sync`.
    unsafe {
        with_connection(
            "remove_contact_sync",
            meta_backend,
            cancellable,
            error,
            |sync, error| ops::remove_contact(sync, uid, error),
        )
    }
}

// ---------------------------------------------------------------------------
// the shared shape

/// Runs `f` under cancellable scope [`observe`] and retries on 401 authentication
/// failure after refreshing OAuth 2.0 credentials via [`retry_on_authentication_failure`].
///
/// # Safety
///
/// Out-parameters, `cancellable`, and `error` must be valid per EDS vfunc contracts.
///
/// [`observe`]: jmap_backend_core::cancel::observe
unsafe fn with_connection(
    context: &str,
    meta_backend: *mut EBookMetaBackend,
    cancellable: *mut GCancellable,
    error: *mut *mut GError,
    mut f: impl FnMut(&BookSync, *mut *mut GError) -> gboolean,
) -> gboolean {
    unsafe {
        guard_bool(context, error, || {
            let _cancel = observe(cancellable);
            let Some(session) = instance(meta_backend).and_then(JmapBookBackend::session) else {
                return fail_offline(error);
            };
            // Lock held across attempts; token refresh modifies client credentials in place.
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

/// JMAP types monitored for address book changes.
const PUSHED_TYPES: &[&str] = &["ContactCard", "AddressBook"];

/// Subscribes to JMAP EventSource push notifications if supported by server.
///
/// # Safety
///
/// `meta_backend` must be a valid, live GObject instance.
unsafe fn start_push(meta_backend: *mut EBookMetaBackend, sync: &BookSync) -> Option<PushRefresh> {
    // SAFETY: Valid GObject-derived instance referenced by EDS.
    // The trampoline receives this pointer and casts it to the origin type.
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

/// The longest time that a push refresh waits for the main loop.
const MAIN_LOOP_WAIT: Duration = Duration::from_secs(5);

/// Queues a push refresh as a custom operation, one at a time.
///
/// EDS's own scheduled refresh can deadlock the factory. The queue is serial,
/// so that refresh starts when the previous operation returns, possibly before
/// the main loop runs the finish callback of that operation. The finish
/// callback holds the lock of a book view and the view watcher then waits for
/// the cache lock. The refresh holds the cache lock and waits for the lock of
/// the view.
///
/// # Safety
///
/// `object` must be a live `EBookMetaBackend` instance.
unsafe extern "C" fn schedule_refresh(object: *mut gobject_sys::GObject) {
    let meta_backend = object.cast::<EBookMetaBackend>();
    // SAFETY: forwarded to the caller by this function's own contract.
    let Some(backend) = (unsafe { instance(meta_backend) }) else {
        return;
    };
    if backend.refresh_queued.swap(true, Ordering::AcqRel) {
        return;
    }
    // SAFETY: the backend is live. The operation takes its own reference.
    unsafe {
        e_book_backend_schedule_custom_operation(
            meta_backend.cast(),
            ptr::null_mut(),
            Some(refresh_after_main_loop),
            ptr::null_mut(),
            None,
        );
    }
}

/// Refreshes in the operation thread when the main loop has run the callbacks
/// of the earlier operations. Logs a failure and does not report it.
///
/// # Safety
///
/// `book_backend` must be a live instance of this type, and `cancellable` valid
/// or NULL.
unsafe extern "C" fn refresh_after_main_loop(
    book_backend: *mut EBookBackend,
    _user_data: gpointer,
    cancellable: *mut GCancellable,
    _error: *mut *mut GError,
) {
    guard("refresh_after_main_loop", (), || {
        let meta_backend = book_backend.cast::<EBookMetaBackend>();
        // SAFETY: the contract of this function.
        if let Some(backend) = unsafe { instance(meta_backend) } {
            backend.refresh_queued.store(false, Ordering::Release);
        }
        // SAFETY: NULL is the global default context, which the factory iterates.
        if !unsafe { wait_for_queued(ptr::null_mut(), MAIN_LOOP_WAIT) } {
            tracing::warn!("the main loop is busy; refreshing the address book without the wait");
        }
        let mut error: *mut GError = ptr::null_mut();
        // SAFETY: the backend is live, and `error` is a local out-parameter.
        unsafe {
            if e_book_meta_backend_refresh_sync(meta_backend, cancellable, &mut error) == GFALSE {
                let message = error
                    .as_ref()
                    .and_then(|error| read_string(error.message))
                    .unwrap_or_default();
                tracing::warn!(
                    error = message.as_str(),
                    "the push refresh of the address book failed"
                );
            }
            g_clear_error(&mut error);
        }
    });
}

/// Refreshes OAuth 2.0 access token and updates client credentials and push headers.
///
/// # Safety
///
/// `meta_backend` and `cancellable` must be valid per EDS contracts.
unsafe fn refresh_credentials(
    meta_backend: *mut EBookMetaBackend,
    sync: &BookSync,
    cancellable: *mut GCancellable,
) -> bool {
    // SAFETY: Valid EBackend instance or detached test instance. Source is borrowed.
    let source = unsafe { backend_source(meta_backend.cast()) };
    // SAFETY: NULL or a valid `ESource`, which is what `source_uses_oauth2`
    // and `access_token` each ask for. A non-OAuth2 account's 401 means the
    // stored password is wrong, which a re-fetch would only reproduce.
    if source.is_null() || !unsafe { source_uses_oauth2(source) } {
        return false;
    }

    // SAFETY: a valid `ESource`, checked non-NULL above, and a cancellable
    // satisfying the contract by this function's own.
    match unsafe { access_token(source, cancellable) } {
        Ok(token) => {
            tracing::debug!("refreshed the address book connection's OAuth 2.0 access token");
            sync.client().set_credentials(Credentials::bearer(token));
            if let Some(header) = sync.client().authorization_header()
                // SAFETY: forwarded from this function's own contract.
                && let Some(backend) = unsafe { instance(meta_backend) }
            {
                backend.refresh_push_headers(vec![("Authorization".to_owned(), header)]);
            }
            true
        }
        Err(failure) => {
            // Debug trace only; EDS handles persistent authentication failure.
            tracing::debug!(
                ?failure,
                "could not refresh the address book connection's OAuth 2.0 access token"
            );
            false
        }
    }
}

/// Returns the Rust view of an instance pointer from EDS.
///
/// # Safety
///
/// `meta_backend` must be NULL or point at an instance of this type. EDS only
/// dispatches a class's vfuncs on instances of that class, so the cast is the
/// same one every `G_DEFINE_TYPE` backend makes.
unsafe fn instance<'a>(meta_backend: *mut EBookMetaBackend) -> Option<&'a JmapBookBackend> {
    unsafe { meta_backend.cast::<JmapBookBackend>().as_ref() }
}

/// Adds the server versions in the cached extras to the infos of `list`.
///
/// # Safety
///
/// `backend` must point to a valid instance, `list` must be null or a list that
/// `marshal::info_list` made, and `cancellable` must be valid or null.
unsafe fn keep_cached_versions(
    backend: *mut EBookMetaBackend,
    list: *mut GSList,
    cancellable: *mut GCancellable,
) {
    if list.is_null() {
        return;
    }
    // SAFETY: `backend` is a valid instance. Returned cache is a new reference or null.
    let cache = unsafe { e_book_meta_backend_ref_cache(backend) };
    if cache.is_null() {
        return;
    }
    let cached = |uid: *const gchar| {
        let mut extra: *mut gchar = ptr::null_mut();
        // SAFETY: the cache reference and `uid` are valid; the call gives an owned string.
        unsafe {
            if e_book_cache_get_contact_extra(cache, uid, &mut extra, cancellable, ptr::null_mut())
                == GFALSE
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

/// Sets `E_CLIENT_ERROR_REPOSITORY_OFFLINE` when backend is disconnected.
///
/// # Safety
///
/// As [`set_raw_gerror`].
unsafe fn fail_offline(error: *mut *mut GError) -> gboolean {
    let message = cstring_lossy("the JMAP address book is not connected");
    // SAFETY: the code is one of the enum's own values and the message is
    // copied by the call; `error` satisfies the contract by this function's.
    unsafe {
        set_raw_gerror(
            error,
            e_client_error_create(E_CLIENT_ERROR_REPOSITORY_OFFLINE, message.as_ptr()),
        );
    }
    GFALSE
}

/// Resolves destination host and port for `EBackendClass::get_destination_address`.
unsafe extern "C" fn get_destination_address(
    backend: *mut EBackend,
    host: *mut *mut gchar,
    port: *mut guint16,
) -> gboolean {
    guard("get_destination_address", GFALSE, || {
        // `(transfer none)`, and NULL only for a backend EDS did not
        // construct from a source.
        // SAFETY: EDS provides a valid backend instance alive for the call.
        let source = unsafe { e_backend_get_source(backend) };
        if source.is_null() {
            return GFALSE;
        }

        // SAFETY: a valid account source, only read from, alive for the call.
        let Some((address_host, address_port)) = (unsafe { destination_address(source) }) else {
            return GFALSE;
        };

        tracing::debug!(
            host = address_host.as_str(),
            port = address_port,
            "resolved destination address for address book backend"
        );

        // SAFETY: `host` and `port` are out-parameters; caller takes ownership of host string.
        unsafe {
            *host = dup_string(&address_host);
            *port = address_port;
        }
        GTRUE
    })
}

/// The property name for required contact fields in EDS.
pub const REQUIRED_FIELDS_PROPERTY: &CStr = c"required-fields";
/// The property name for supported contact fields in EDS.
pub const SUPPORTED_FIELDS_PROPERTY: &CStr = c"supported-fields";

/// Fields mapped between JSContact and EDS (RFC 9553, RFC 9610, docs/VCARD-MAPPING.md).
pub const SUPPORTED_FIELD_IDS: &[EContactField] = &[
    E_CONTACT_UID,
    E_CONTACT_FILE_AS,
    E_CONTACT_FULL_NAME,
    E_CONTACT_GIVEN_NAME,
    E_CONTACT_FAMILY_NAME,
    E_CONTACT_NICKNAME,
    E_CONTACT_EMAIL_1,
    E_CONTACT_EMAIL_2,
    E_CONTACT_EMAIL_3,
    E_CONTACT_EMAIL_4,
    E_CONTACT_ADDRESS_LABEL_HOME,
    E_CONTACT_ADDRESS_LABEL_WORK,
    E_CONTACT_ADDRESS_LABEL_OTHER,
    E_CONTACT_PHONE_ASSISTANT,
    E_CONTACT_PHONE_BUSINESS,
    E_CONTACT_PHONE_BUSINESS_2,
    E_CONTACT_PHONE_BUSINESS_FAX,
    E_CONTACT_PHONE_CALLBACK,
    E_CONTACT_PHONE_CAR,
    E_CONTACT_PHONE_COMPANY,
    E_CONTACT_PHONE_HOME,
    E_CONTACT_PHONE_HOME_2,
    E_CONTACT_PHONE_HOME_FAX,
    E_CONTACT_PHONE_ISDN,
    E_CONTACT_PHONE_MOBILE,
    E_CONTACT_PHONE_OTHER,
    E_CONTACT_PHONE_OTHER_FAX,
    E_CONTACT_PHONE_PAGER,
    E_CONTACT_PHONE_PRIMARY,
    E_CONTACT_PHONE_RADIO,
    E_CONTACT_PHONE_TELEX,
    E_CONTACT_PHONE_TTYTDD,
    E_CONTACT_ORG,
    E_CONTACT_ORG_UNIT,
    E_CONTACT_OFFICE,
    E_CONTACT_TITLE,
    E_CONTACT_ROLE,
    E_CONTACT_MANAGER,
    E_CONTACT_ASSISTANT,
    E_CONTACT_HOMEPAGE_URL,
    E_CONTACT_BLOG_URL,
    E_CONTACT_CATEGORIES,
    E_CONTACT_CALENDAR_URI,
    E_CONTACT_FREEBUSY_URL,
    E_CONTACT_VIDEO_URL,
    E_CONTACT_SPOUSE,
    E_CONTACT_NOTE,
    E_CONTACT_IM_AIM_HOME_1,
    E_CONTACT_IM_AIM_HOME_2,
    E_CONTACT_IM_AIM_HOME_3,
    E_CONTACT_IM_AIM_WORK_1,
    E_CONTACT_IM_AIM_WORK_2,
    E_CONTACT_IM_AIM_WORK_3,
    E_CONTACT_IM_GROUPWISE_HOME_1,
    E_CONTACT_IM_GROUPWISE_HOME_2,
    E_CONTACT_IM_GROUPWISE_HOME_3,
    E_CONTACT_IM_GROUPWISE_WORK_1,
    E_CONTACT_IM_GROUPWISE_WORK_2,
    E_CONTACT_IM_GROUPWISE_WORK_3,
    E_CONTACT_IM_JABBER_HOME_1,
    E_CONTACT_IM_JABBER_HOME_2,
    E_CONTACT_IM_JABBER_HOME_3,
    E_CONTACT_IM_JABBER_WORK_1,
    E_CONTACT_IM_JABBER_WORK_2,
    E_CONTACT_IM_JABBER_WORK_3,
    E_CONTACT_IM_YAHOO_HOME_1,
    E_CONTACT_IM_YAHOO_HOME_2,
    E_CONTACT_IM_YAHOO_HOME_3,
    E_CONTACT_IM_YAHOO_WORK_1,
    E_CONTACT_IM_YAHOO_WORK_2,
    E_CONTACT_IM_YAHOO_WORK_3,
    E_CONTACT_IM_MSN_HOME_1,
    E_CONTACT_IM_MSN_HOME_2,
    E_CONTACT_IM_MSN_HOME_3,
    E_CONTACT_IM_MSN_WORK_1,
    E_CONTACT_IM_MSN_WORK_2,
    E_CONTACT_IM_MSN_WORK_3,
    E_CONTACT_IM_ICQ_HOME_1,
    E_CONTACT_IM_ICQ_HOME_2,
    E_CONTACT_IM_ICQ_HOME_3,
    E_CONTACT_IM_ICQ_WORK_1,
    E_CONTACT_IM_ICQ_WORK_2,
    E_CONTACT_IM_ICQ_WORK_3,
    E_CONTACT_ADDRESS,
    E_CONTACT_ADDRESS_HOME,
    E_CONTACT_ADDRESS_WORK,
    E_CONTACT_ADDRESS_OTHER,
    E_CONTACT_CATEGORY_LIST,
    E_CONTACT_PHOTO,
    E_CONTACT_NAME,
    E_CONTACT_EMAIL,
    E_CONTACT_IM_AIM,
    E_CONTACT_IM_GROUPWISE,
    E_CONTACT_IM_JABBER,
    E_CONTACT_IM_YAHOO,
    E_CONTACT_IM_MSN,
    E_CONTACT_IM_ICQ,
    E_CONTACT_BIRTH_DATE,
    E_CONTACT_ANNIVERSARY,
    E_CONTACT_IM_GADUGADU_HOME_1,
    E_CONTACT_IM_GADUGADU_HOME_2,
    E_CONTACT_IM_GADUGADU_HOME_3,
    E_CONTACT_IM_GADUGADU_WORK_1,
    E_CONTACT_IM_GADUGADU_WORK_2,
    E_CONTACT_IM_GADUGADU_WORK_3,
    E_CONTACT_IM_GADUGADU,
    E_CONTACT_TEL,
    E_CONTACT_IM_SKYPE_HOME_1,
    E_CONTACT_IM_SKYPE_HOME_2,
    E_CONTACT_IM_SKYPE_HOME_3,
    E_CONTACT_IM_SKYPE_WORK_1,
    E_CONTACT_IM_SKYPE_WORK_2,
    E_CONTACT_IM_SKYPE_WORK_3,
    E_CONTACT_IM_SKYPE,
    E_CONTACT_IM_GOOGLE_TALK_HOME_1,
    E_CONTACT_IM_GOOGLE_TALK_HOME_2,
    E_CONTACT_IM_GOOGLE_TALK_HOME_3,
    E_CONTACT_IM_GOOGLE_TALK_WORK_1,
    E_CONTACT_IM_GOOGLE_TALK_WORK_2,
    E_CONTACT_IM_GOOGLE_TALK_WORK_3,
    E_CONTACT_IM_GOOGLE_TALK,
    E_CONTACT_IM_MATRIX_HOME_1,
    E_CONTACT_IM_MATRIX_HOME_2,
    E_CONTACT_IM_MATRIX_HOME_3,
    E_CONTACT_IM_MATRIX_WORK_1,
    E_CONTACT_IM_MATRIX_WORK_2,
    E_CONTACT_IM_MATRIX_WORK_3,
    E_CONTACT_IM_MATRIX,
];

/// Comma-separated list of field names supported by this backend.
pub fn supported_fields() -> &'static str {
    static SUPPORTED: OnceLock<String> = OnceLock::new();
    SUPPORTED.get_or_init(|| {
        let mut names = Vec::with_capacity(SUPPORTED_FIELD_IDS.len());
        for &field in SUPPORTED_FIELD_IDS {
            // SAFETY: field is a valid EContactField constant; e_contact_field_name returns a static C string.
            unsafe {
                let name_ptr = e_contact_field_name(field);
                if !name_ptr.is_null() {
                    let cstr = CStr::from_ptr(name_ptr);
                    if let Ok(name) = cstr.to_str() {
                        names.push(name);
                    }
                }
            }
        }
        names.join(",")
    })
}

/// Comma-separated list of required field names. JSContact requires no specific contact field.
pub fn required_fields() -> &'static str {
    ""
}

/// Answers backend property values, advertising supported-fields and required-fields.
unsafe extern "C" fn get_backend_property(
    backend: *mut EBookBackend,
    prop_name: *const gchar,
) -> *mut gchar {
    // SAFETY: EDS contract: valid backend instance and NUL-terminated property name.
    unsafe {
        guard("get_backend_property", ptr::null_mut(), || {
            if !prop_name.is_null() {
                let name = CStr::from_ptr(prop_name);
                if name == REQUIRED_FIELDS_PROPERTY {
                    return dup_string(required_fields());
                }
                if name == SUPPORTED_FIELDS_PROPERTY {
                    return dup_string(supported_fields());
                }
            }

            match parent_class()
                .and_then(|class| class.parent_class.parent_class.impl_get_backend_property)
            {
                Some(chain_up) => chain_up(backend, prop_name),
                None => ptr::null_mut(),
            }
        })
    }
}
