// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether the keyring an account's credentials live in is locked.
//!
//! ## The one question EDS cannot be asked
//!
//! `docs/ROADMAP.md` item 17 asked for a dead secret store to surface a
//! message instead of another consent window, and its (b)/(c) halves are
//! answered by classifying the `GError`
//! `e_source_get_oauth2_access_token_sync` sets — see
//! [`crate::oauth2::is_secret_store_failure`]. Its (a) half, a *locked*
//! login keyring, cannot be, and the reason is structural rather than an
//! oversight anywhere:
//!
//! `e_secret_store_lookup_sync` calls `secret_password_lookup_sync`, whose
//! search finds the matching item in the collection's **locked** list and
//! tries to unlock it with a D-Bus prompt. When that prompt cannot be shown
//! (no display) or the user dismisses it, libsecret's own documented
//! contract for `secret_service_prompt_sync` is that it "returns %NULL if
//! the prompt was dismissed **or an error occurred**" — a refused prompt is
//! not a `GError` by that API's design. So the lookup answers `TRUE` with a
//! `NULL` secret, and `eos_lookup_token_sync` turns that into exactly the
//! `G_IO_ERROR_NOT_FOUND` it gives for "nobody has ever consented". The
//! information "the collection exists but is locked" never reaches the
//! caller at all, and no classification of an error that was never set can
//! recover it.
//!
//! This module asks the secret service directly instead, over the same
//! `org.freedesktop.secrets` D-Bus API libsecret itself speaks.
//!
//! ## Why GDBus rather than libsecret
//!
//! The obvious spelling is `secret_collection_get_locked()`, and it was not
//! taken: libsecret is not among the libraries `eds-sys` probes, so using it
//! would mean a new pkg-config dependency, a new bindgen surface, a new
//! `.deb` build-dependency, and a new package in both `ci/install-deps.sh`
//! *and* the `eds-version-matrix` job's Fedora `dnf` line. `gio-sys` 0.22 is
//! already a dependency of this crate, GDBus needs no headers of its own,
//! and the Secret Service D-Bus interface is the very thing libsecret is a
//! wrapper *around* — so this is the same trade [`crate::resolver`] made
//! when it reached for `g_resolver_lookup_service()` rather than adding a
//! DNS crate: an already-linked library, no new build environment to keep in
//! step across three CI images.
//!
//! Two calls, both from the Secret Service API's own specification:
//! `org.freedesktop.Secret.Service.ReadAlias("default")` for the collection
//! EDS stores into — `e_secret_store_store_sync` passes
//! `SECRET_COLLECTION_DEFAULT` for anything stored permanently, read from
//! evolution-data-server 3.52.3's `src/libedataserver/e-secret-store.c`
//! rather than assumed — and then that collection's `Locked` property.
//!
//! ## Never activate anything
//!
//! Every call here passes [`G_DBUS_CALL_FLAGS_NO_AUTO_START`]. A question
//! asked only to make an *already-failed* connect's message better must not
//! be the thing that starts a daemon: in `evolution-source-registry`, where
//! the address book and calendar backends run, D-Bus-activating
//! `org.freedesktop.secrets` is a 25-second timeout when it cannot start
//! (`docs/ROADMAP.md` item 18) and a side effect on the session when it can.
//! With the flag, a service that is not running answers "cannot determine",
//! which is [`None`] here and no behaviour change at all at the call site.
//!
//! Everything else answers [`None`] for the same reason: no session bus, no
//! `default` alias, a reply of an unexpected shape. This module can only
//! ever *improve* the diagnosis of a failure that has already happened, so
//! "do not know" and "not locked" are the same safe answer and the caller
//! treats them alike.

use std::ffi::{CStr, CString};
use std::ptr;

use gio_sys::{
    G_BUS_TYPE_SESSION, G_DBUS_CALL_FLAGS_NO_AUTO_START, GDBusConnection, g_bus_get_sync,
    g_dbus_connection_call_sync,
};
use glib_sys::{
    GError, GFALSE, GVariant, g_error_free, g_variant_get_boolean, g_variant_get_child_value,
    g_variant_get_string, g_variant_get_variant, g_variant_is_of_type, g_variant_new_string,
    g_variant_n_children, g_variant_new_tuple, g_variant_type_free, g_variant_type_new,
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

/// The message bus itself. Unlike everything else this module talks to, this
/// peer is unconditionally present: a process that has a session bus has the
/// bus daemon, so asking it a question can neither activate anything nor
/// fail for want of a service.
const BUS_NAME: &CStr = c"org.freedesktop.DBus";
const BUS_PATH: &CStr = c"/org/freedesktop/DBus";
const BUS_INTERFACE: &CStr = c"org.freedesktop.DBus";

/// The alias `SECRET_COLLECTION_DEFAULT` is spelled with on the wire — the
/// login keyring on an ordinary GNOME session, and the collection
/// `e_secret_store_store_sync` writes a permanently-stored secret to.
const DEFAULT_ALIAS: &CStr = c"default";

/// The Secret Service's answer for an alias nothing is registered under. A
/// path is only a path if it names something.
const NO_SUCH_OBJECT: &str = "/";

/// Deliberately far below GDBus's own 25-second default. This is a
/// diagnostic afterthought on a connect that has already failed, so a secret
/// service that is running but wedged must cost the user a moment, not the
/// better part of a minute — and answering "do not know" after that moment
/// is the same behaviour as not asking at all.
const TIMEOUT_MS: i32 = 5_000;

/// Whether the keyring EDS stores this session's credentials in is locked,
/// or [`None`] where that could not be determined — see the module docs on
/// why those two are the same answer to every caller.
pub fn default_collection_is_locked() -> Option<bool> {
    // SAFETY: `g_bus_get_sync` takes no borrowed arguments; the connection it
    // answers with is a reference this scope owns and releases below, and
    // every helper it is passed to only makes calls on it.
    unsafe {
        let connection = session_bus()?;
        let locked =
            default_alias_path(connection).and_then(|path| collection_is_locked(connection, &path));
        g_object_unref(connection.cast());
        locked
    }
}

/// A reference to the session bus, or [`None`] where there is no session bus
/// to be had — a backend started outside a session, or one whose bus has
/// gone away.
///
/// # Safety
///
/// None beyond the call itself; the returned reference is the caller's to
/// release.
unsafe fn session_bus() -> Option<*mut GDBusConnection> {
    let mut error: *mut GError = ptr::null_mut();
    // SAFETY: a valid bus type, no cancellable, and a writable out-parameter
    // for the error. The connection comes back `(transfer full)`.
    let connection = unsafe { g_bus_get_sync(G_BUS_TYPE_SESSION, ptr::null_mut(), &mut error) };
    if connection.is_null() {
        // SAFETY: NULL or a `GError` this call owns; read then freed.
        unsafe { report(error, "no session bus to ask about the secret store") };
        return None;
    }
    Some(connection)
}

/// One method call, with this module's fixed policy: never auto-start, a
/// short timeout, a reply type GDBus itself checks, and any failure at all
/// answered [`None`].
///
/// # Safety
///
/// `connection` must be a live `GDBusConnection`, and `parameters` a
/// `GVariant` tuple this call may consume — `g_dbus_connection_call_sync`
/// sinks a floating reference, which is what every caller here hands it.
unsafe fn call(
    connection: *mut GDBusConnection,
    destination: &CStr,
    object_path: &CStr,
    interface: &CStr,
    method: &CStr,
    parameters: *mut GVariant,
    reply_type: &CStr,
) -> Option<*mut GVariant> {
    // SAFETY: a NUL-terminated GVariant type string; freed below whatever the
    // call answers.
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

/// Logs `error` at debug and frees it. A secret service that cannot be
/// reached is the ordinary case on a machine that has none, so this is never
/// louder than that.
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
    // SAFETY: a NUL-terminated string for the alias; the resulting floating
    // `GVariant`s are sunk by `g_variant_new_tuple` and then by the call.
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
    // An object path with an interior NUL is not one any D-Bus reply could
    // have carried, so this is unreachable rather than a real case — and
    // refusing is the same safe "do not know" everything else here answers.
    let object_path = CString::new(object_path).ok()?;
    // SAFETY: NUL-terminated strings for the interface and property names;
    // the floating `GVariant`s are sunk by `g_variant_new_tuple` and the
    // tuple by the call.
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
        // `(v)`, so child 0 exists and is a box; what is *inside* the box is
        // the service's choice, so it is checked rather than assumed.
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

/// Whether this machine has a secret service *at all*, meaning running now or
/// startable on demand, or [`None`] where that could not be determined.
///
/// This is a different question from [`default_collection_is_locked`], and it
/// exists for a different caller. A *locked* store is one that holds the
/// account's token and will hand it over once unlocked, so the right answer
/// is to say so and wait. A store that is not there is one that could not
/// hold a token even if the user signed in again, which makes offering them
/// a sign-in window an invitation to do work that cannot be saved: the
/// consent completes, the token has nowhere to go, and the next fetch asks
/// again. Erroring out is the honest outcome; see [`crate::oauth2`] for where
/// that decision is taken.
///
/// Both questions go to the **message bus**, never to the secret service, so
/// this keeps the module's "never activate anything" promise (see the module
/// docs) while still distinguishing "not running" from "not installed":
///
/// - `NameHasOwner` answers whether it is running *now*. A store that is up
///   is available, whatever state its collections are in.
/// - `ListActivatableNames` answers whether the bus *could* start one. A
///   session where the keyring simply has not been needed yet must keep
///   behaving exactly as it did before this function existed, and this is
///   what keeps it doing so. The alternative, treating "not running" as
///   "not there", would refuse sign-in on a perfectly ordinary desktop.
///
/// Only `Some(false)`, both questions answered and both negative, means no
/// store. Anything unclear is [`None`], which callers must treat as "carry on
/// as before".
pub fn service_is_available() -> Option<bool> {
    // SAFETY: the connection is a reference this scope owns and releases, and
    // every helper it is passed to only makes calls on it.
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
    // SAFETY: a NUL-terminated bus name; the floating `GVariant` is sunk by
    // `g_variant_new_tuple` and the tuple by the call.
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
    // SAFETY: no arguments to sink; the reply is `(as)` per the type GDBus
    // checked, so child 0 is an array of strings and each of its children is
    // a string.
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
            if read_string(g_variant_get_string(entry, ptr::null_mut())).as_deref() == Some(wanted) {
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
