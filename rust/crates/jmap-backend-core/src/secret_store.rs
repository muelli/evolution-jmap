// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether the keyring that holds the credentials of an account is locked.
//!
//! ## The one question EDS cannot answer
//!
//! A failed secret store must show a message, not one more consent window.
//! The `GError` that `e_source_get_oauth2_access_token_sync` sets identifies
//! two of the three causes of this failure (see
//! `crate::oauth2::is_secret_store_failure`). It cannot identify the third
//! cause, a *locked* login keyring. The cause of this is the design, not an
//! oversight:
//!
//! `e_secret_store_lookup_sync` calls `secret_password_lookup_sync`. Its
//! search finds the matching item in the **locked** list of the collection
//! and tries to unlock it with a D-Bus prompt. The prompt can fail to show
//! (no display), or the user can dismiss it. The documented contract of
//! libsecret for `secret_service_prompt_sync` is that it "returns %NULL if
//! the prompt was dismissed **or an error occurred**". Thus, by design, a
//! refused prompt is not a `GError`. The lookup answers `TRUE` with a `NULL`
//! secret, and `eos_lookup_token_sync` changes that into the same
//! `G_IO_ERROR_NOT_FOUND` that it gives for "nobody has ever consented". The
//! information "the collection exists but is locked" never gets to the
//! caller. No classification of an error that was never set can recover it.
//!
//! Thus this module asks the secret service directly, over the
//! `org.freedesktop.secrets` D-Bus API that libsecret also uses.
//!
//! ## Why GDBus and not libsecret
//!
//! The obvious call is `secret_collection_get_locked()`, but this module does
//! not use it. `eds-sys` does not probe libsecret. Thus libsecret would need
//! a new pkg-config dependency, a new bindgen surface, a new `.deb`
//! build-dependency, and a new package in both `ci/install-deps.sh` *and* the
//! Fedora `dnf` line of the `eds-version-matrix` job. This crate already
//! uses `gio-sys` 0.22, and GDBus needs no headers of its own. libsecret is
//! only a wrapper *around* the Secret Service D-Bus interface. This is the
//! same trade that [`crate::resolver`] made when it used
//! `g_resolver_lookup_service()` and not a DNS crate: an already-linked
//! library, and no new build environment to keep in step across three CI
//! images.
//!
//! The module makes two calls, both from the Secret Service API
//! specification:
//!
//! 1. `org.freedesktop.Secret.Service.ReadAlias("default")` gives the
//!    collection that EDS stores into. `e_secret_store_store_sync` passes
//!    `SECRET_COLLECTION_DEFAULT` for all permanent secrets. This fact comes
//!    from `src/libedataserver/e-secret-store.c` in evolution-data-server
//!    3.52.3, not from an assumption.
//! 2. The `Locked` property of that collection gives the answer.
//!
//! ## Never activate anything
//!
//! Every call here passes [`G_DBUS_CALL_FLAGS_NO_AUTO_START`]. This question
//! only improves the message of a connect that *already failed*, so it must
//! not start a daemon. In `evolution-source-registry`, where the address book
//! and calendar backends run, D-Bus activation of `org.freedesktop.secrets`
//! causes a 25-second timeout when the service cannot start. When it can
//! start, the activation has a side effect on the session.
//! With the flag, a service that is not running answers "cannot determine".
//! That is [`None`] here, and it causes no behaviour change at the call site.
//!
//! All other failures answer [`None`] for the same reason: no session bus, no
//! `default` alias, or a reply of an unexpected shape. This module can only
//! *improve* the diagnosis of a failure that already occurred. Thus "do not
//! know" and "not locked" are the same safe answer, and the caller treats
//! them the same.

use std::ffi::{CStr, CString};
use std::ptr;

use gio_sys::{
    G_BUS_TYPE_SESSION, G_DBUS_CALL_FLAGS_NO_AUTO_START, GDBusConnection, g_bus_get_sync,
    g_dbus_connection_call_sync,
};
use glib_sys::{
    GError, GFALSE, GVariant, g_error_free, g_variant_get_boolean, g_variant_get_child_value,
    g_variant_get_string, g_variant_get_variant, g_variant_is_of_type, g_variant_n_children,
    g_variant_new_string, g_variant_new_tuple, g_variant_type_free, g_variant_type_new,
    g_variant_unref,
};
use gobject_sys::g_object_unref;

use crate::marshal::read_string;

/// The Secret Service, as the specification names it.
const SERVICE_NAME: &CStr = c"org.freedesktop.secrets";
const SERVICE_PATH: &CStr = c"/org/freedesktop/secrets";
const SERVICE_INTERFACE: &CStr = c"org.freedesktop.Secret.Service";
const COLLECTION_INTERFACE: &CStr = c"org.freedesktop.Secret.Collection";
const PROPERTIES_INTERFACE: &CStr = c"org.freedesktop.DBus.Properties";

/// The message bus. Unlike all other peers of this module, this peer is
/// always present: a process that has a session bus has the bus daemon.
/// Thus a question to it cannot activate anything, and cannot fail because
/// a service is missing.
const BUS_NAME: &CStr = c"org.freedesktop.DBus";
const BUS_PATH: &CStr = c"/org/freedesktop/DBus";
const BUS_INTERFACE: &CStr = c"org.freedesktop.DBus";

/// The wire name of the alias `SECRET_COLLECTION_DEFAULT`. On an ordinary
/// GNOME session, it is the login keyring. It is also the collection that
/// `e_secret_store_store_sync` writes a permanent secret to.
const DEFAULT_ALIAS: &CStr = c"default";

/// The answer of the Secret Service for an alias with no registered object.
/// Only a path that names an object is a real path.
const NO_SUCH_OBJECT: &str = "/";

/// Intentionally much shorter than the 25-second default of GDBus. This is
/// only a diagnostic for a connect that already failed. Thus a secret
/// service that runs but does not respond must cost the user only a moment,
/// not almost a minute. To answer "do not know" after that moment is the
/// same behaviour as not to ask.
const TIMEOUT_MS: i32 = 5_000;

/// Whether the keyring where EDS stores the credentials of this session is
/// locked, or [`None`] if this is not known. The module docs tell why
/// [`None`] and `Some(false)` are the same answer to every caller.
pub fn default_collection_is_locked() -> Option<bool> {
    // SAFETY: `g_bus_get_sync` takes no borrowed arguments. This scope owns
    // the connection reference that it returns, and releases it below. Every
    // helper that gets the connection only makes calls on it.
    unsafe {
        let connection = session_bus()?;
        let locked =
            default_alias_path(connection).and_then(|path| collection_is_locked(connection, &path));
        g_object_unref(connection.cast());
        locked
    }
}

/// A reference to the session bus, or [`None`] if no session bus is
/// available. Examples: a backend that started outside a session, or a
/// backend whose bus stopped.
///
/// # Safety
///
/// No conditions other than the call itself. The caller must release the
/// returned reference.
unsafe fn session_bus() -> Option<*mut GDBusConnection> {
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a valid bus type, no cancellable, and a writable out-parameter
    // for the error. The returned connection is `(transfer full)`.
    let connection = unsafe { g_bus_get_sync(G_BUS_TYPE_SESSION, ptr::null_mut(), &mut error) };
    if connection.is_null() {
        // SAFETY: NULL or a `GError` this call owns; read then freed.
        unsafe { report(error, "no session bus to ask about the secret store") };
        return None;
    }
    Some(connection)
}

/// One method call, with the fixed policy of this module: no auto-start, a
/// short timeout, a reply type that GDBus checks, and [`None`] for all
/// failures.
///
/// # Safety
///
/// `connection` must be a live `GDBusConnection`. `parameters` must be a
/// `GVariant` tuple that this call can consume.
/// `g_dbus_connection_call_sync` sinks a floating reference, and every
/// caller here gives it one.
unsafe fn call(
    connection: *mut GDBusConnection,
    destination: &CStr,
    object_path: &CStr,
    interface: &CStr,
    method: &CStr,
    parameters: *mut GVariant,
    reply_type: &CStr,
) -> Option<*mut GVariant> {
    // SAFETY: a NUL-terminated GVariant type string. The code below frees it,
    // whatever the call answers.
    let reply_type = unsafe { g_variant_type_new(reply_type.as_ptr()) };
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a live connection, NUL-terminated names, a tuple this call
    // takes, a valid reply type, and a writable out-parameter for the error.
    // A non-NULL reply is `(transfer full)`.
    let reply = unsafe {
        g_dbus_connection_call_sync(
            connection,
            destination.as_ptr(),
            object_path.as_ptr(),
            interface.as_ptr(),
            method.as_ptr(),
            parameters,
            reply_type,
            G_DBUS_CALL_FLAGS_NO_AUTO_START,
            TIMEOUT_MS,
            ptr::null_mut(),
            &mut error,
        )
    };
    // SAFETY: the type this scope allocated above, used for the last time.
    unsafe { g_variant_type_free(reply_type) };

    if reply.is_null() {
        // SAFETY: NULL or a `GError` this call owns; read then freed.
        unsafe { report(error, "the secret service did not answer") };
        return None;
    }
    Some(reply)
}

/// Logs `error` at debug level and frees it. On a machine with no secret
/// service, an unreachable service is the ordinary case. Thus this log is
/// never louder than debug.
///
/// # Safety
///
/// `error` must be NULL or a `GError` this call may free.
unsafe fn report(error: *mut GError, what: &str) {
    if error.is_null() {
        return;
    }
    // SAFETY: a valid `GError`; its message is owned by the struct and read
    // before the struct is freed.
    unsafe {
        tracing::debug!(message = ?read_string((*error).message), "{what}");
        g_error_free(error);
    }
}

/// The object path of the collection the `default` alias names.
///
/// # Safety
///
/// `connection` must be a live `GDBusConnection`.
unsafe fn default_alias_path(connection: *mut GDBusConnection) -> Option<String> {
    // SAFETY: a NUL-terminated string for the alias. `g_variant_new_tuple`
    // sinks the floating `GVariant`s, and then the call sinks the tuple.
    let path = unsafe {
        let arguments = [g_variant_new_string(DEFAULT_ALIAS.as_ptr())];
        let parameters = g_variant_new_tuple(arguments.as_ptr(), arguments.len());
        let reply = call(
            connection,
            SERVICE_NAME,
            SERVICE_PATH,
            SERVICE_INTERFACE,
            c"ReadAlias",
            parameters,
            c"(o)",
        )?;
        // The reply type GDBus checked for is `(o)`, so child 0 exists and is
        // an object path.
        let child = g_variant_get_child_value(reply, 0);
        let path = read_string(g_variant_get_string(child, ptr::null_mut()));
        g_variant_unref(child);
        g_variant_unref(reply);
        path
    };
    path.filter(|path| path != NO_SUCH_OBJECT)
}

/// Whether the collection at `object_path` reports itself locked.
///
/// # Safety
///
/// `connection` must be a live `GDBusConnection`.
unsafe fn collection_is_locked(
    connection: *mut GDBusConnection,
    object_path: &str,
) -> Option<bool> {
    // No D-Bus reply can carry an object path with an interior NUL, so this
    // case is unreachable. A refusal is the same safe "do not know" that all
    // other failures here answer.
    let object_path = CString::new(object_path).ok()?;
    // SAFETY: NUL-terminated strings for the interface and property names.
    // `g_variant_new_tuple` sinks the floating `GVariant`s, and the call
    // sinks the tuple.
    unsafe {
        let arguments = [
            g_variant_new_string(COLLECTION_INTERFACE.as_ptr()),
            g_variant_new_string(c"Locked".as_ptr()),
        ];
        let parameters = g_variant_new_tuple(arguments.as_ptr(), arguments.len());
        let reply = call(
            connection,
            SERVICE_NAME,
            &object_path,
            PROPERTIES_INTERFACE,
            c"Get",
            parameters,
            c"(v)",
        )?;
        // The reply type is `(v)`, so child 0 exists and is a box. The
        // service chooses what is *inside* the box, so the code checks it.
        let boxed = g_variant_get_child_value(reply, 0);
        let value = g_variant_get_variant(boxed);
        let boolean = g_variant_type_new(c"b".as_ptr());
        let locked = (g_variant_is_of_type(value, boolean) != GFALSE)
            .then(|| g_variant_get_boolean(value) != GFALSE);
        g_variant_type_free(boolean);
        g_variant_unref(value);
        g_variant_unref(boxed);
        g_variant_unref(reply);
        locked
    }
}

/// Whether this machine has a secret service *at all* (running now or
/// startable on demand), or [`None`] if this is not known.
///
/// This question is different from [`default_collection_is_locked`], and it
/// is for a different caller. A *locked* store holds the token of the
/// account and gives it after an unlock. Thus the correct answer is to tell
/// the user and wait. A store that is not there cannot hold a token, even
/// after a new sign-in. A sign-in window then asks the user to do work that
/// the store cannot save: the consent completes, the token has no place to
/// go, and the next fetch asks again. An error is the honest result.
/// [`crate::oauth2`] makes that decision.
///
/// Both questions go to the **message bus**, never to the secret service.
/// Thus this function keeps the "never activate anything" promise of the
/// module (see the module docs), but it can still tell "not running" from
/// "not installed":
///
/// - `NameHasOwner` answers whether a store runs *now*. A store that runs is
///   available, whatever the state of its collections.
/// - `ListActivatableNames` answers whether the bus *can* start one. A
///   session that did not need the keyring until now must keep the same
///   behaviour as before this function existed. This check keeps that
///   behaviour. The alternative (to treat "not running" as "not there")
///   would refuse sign-in on a fully ordinary desktop.
///
/// Only `Some(false)` means no store: both questions answered, and both
/// negative. All unclear results are [`None`]. Callers must treat [`None`]
/// as "continue as before".
pub fn service_is_available() -> Option<bool> {
    // SAFETY: this scope owns and releases the connection reference. Every
    // helper that gets the connection only makes calls on it.
    unsafe {
        let connection = session_bus()?;
        let available = bus_name_has_owner(connection, SERVICE_NAME).and_then(|running| {
            if running {
                Some(true)
            } else {
                bus_can_activate(connection, SERVICE_NAME)
            }
        });
        g_object_unref(connection.cast());
        available
    }
}

/// Whether `name` currently has an owner on the bus.
///
/// # Safety
///
/// `connection` must be a live `GDBusConnection`.
unsafe fn bus_name_has_owner(connection: *mut GDBusConnection, name: &CStr) -> Option<bool> {
    // SAFETY: a NUL-terminated bus name. `g_variant_new_tuple` sinks the
    // floating `GVariant`, and the call sinks the tuple.
    unsafe {
        let arguments = [g_variant_new_string(name.as_ptr())];
        let parameters = g_variant_new_tuple(arguments.as_ptr(), arguments.len());
        let reply = call(
            connection,
            BUS_NAME,
            BUS_PATH,
            BUS_INTERFACE,
            c"NameHasOwner",
            parameters,
            c"(b)",
        )?;
        // The reply type GDBus checked for is `(b)`, so child 0 exists and is
        // a boolean.
        let child = g_variant_get_child_value(reply, 0);
        let owned = g_variant_get_boolean(child) != GFALSE;
        g_variant_unref(child);
        g_variant_unref(reply);
        Some(owned)
    }
}

/// Whether the bus could start `name` on demand.
///
/// # Safety
///
/// `connection` must be a live `GDBusConnection`.
unsafe fn bus_can_activate(connection: *mut GDBusConnection, name: &CStr) -> Option<bool> {
    let wanted = name.to_str().ok()?;
    // SAFETY: no arguments to sink. GDBus checked that the reply type is
    // `(as)`. Thus child 0 is an array of strings, and each of its children
    // is a string.
    unsafe {
        let parameters = g_variant_new_tuple(ptr::null(), 0);
        let reply = call(
            connection,
            BUS_NAME,
            BUS_PATH,
            BUS_INTERFACE,
            c"ListActivatableNames",
            parameters,
            c"(as)",
        )?;
        let names = g_variant_get_child_value(reply, 0);
        let mut found = false;
        for index in 0..g_variant_n_children(names) {
            let entry = g_variant_get_child_value(names, index);
            if read_string(g_variant_get_string(entry, ptr::null_mut())).as_deref() == Some(wanted)
            {
                found = true;
            }
            g_variant_unref(entry);
            if found {
                break;
            }
        }
        g_variant_unref(names);
        g_variant_unref(reply);
        Some(found)
    }
}
