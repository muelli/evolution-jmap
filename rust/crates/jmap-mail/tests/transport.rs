// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelJmapTransport`: the second service of a JMAP account. It never reads
//! a folder.
//!
//! Camel gives an account two services, with no way for either to reach the
//! other: a `CamelStore` for the mail it holds, and a `CamelTransport` for the
//! mail it sends. Evolution builds both from two `ESource`s, gives each its
//! own settings object, and connects each independently. So a JMAP transport
//! is not a view of the store. It is a separate service, with its own
//! connection to the same server.
//!
//! These tests check that:
//! - the type is a `CamelTransport`.
//! - it is configured through the account's settings class, not a stock class
//!   with no host.
//! - authenticating it opens a connection.
//! - that connection belongs to the transport, not to the store.
//!
//! `send_to_sync` itself is not here; see `tests/send.rs`. That split matches
//! the source: this file covers the service and its connection, that file
//! covers what a message does with one.

mod common;

use std::ffi::CStr;
use std::ptr;

use common::{Account, Transport};
use eds_sys::{
    CamelNetworkSettings, CamelService, CamelServiceClass, camel_network_settings_set_host,
    camel_network_settings_set_port, camel_network_settings_set_user, camel_service_get_name,
    camel_service_ref_settings, camel_transport_get_type,
};
use glib_sys::{GFALSE, GTRUE};
use gobject_sys::{
    G_TYPE_INVALID, g_type_class_ref, g_type_class_unref, g_type_from_name, g_type_is_a,
};
use jmap_backend_core::source::ConnectTarget;
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_client::Credentials;
use jmap_mail::server::ServerConfig;
use jmap_mail::service::authenticate;
use jmap_mail::settings::settings_type;
use jmap_mail::store::JmapStore;
use jmap_mail::transport::{JmapTransport, transport_type};
use jmap_mock::MockServer;
use jmap_proto::mail::role;

fn config(server: &MockServer) -> ServerConfig {
    ServerConfig {
        target: ConnectTarget::Origin(server.origin().to_owned()),
        user: None,
    }
}

fn seeded() -> MockServer {
    let server = MockServer::builder().start();
    let account_id = server.account_id();
    let state = server.state();
    state
        .lock()
        .unwrap()
        .account_mut(&account_id)
        .unwrap()
        .seed_mailbox("Inbox", Some(role::INBOX));
    server
}

// ---------------------------------------------------------------------------
// the type

/// The parent class is the reason this type exists. `send_to_sync` is declared
/// only on `CamelTransportClass`. `camel_session_add_service` refuses a type
/// that is not a `CamelTransport` when Evolution asks for the sending half of
/// an account.
#[test]
fn the_transport_is_a_camel_transport() {
    let gtype = transport_type();
    assert_ne!(gtype, G_TYPE_INVALID, "the transport type did not register");
    assert_eq!(
        gtype,
        transport_type(),
        "registering it twice made two types"
    );

    // SAFETY: a live GType; the type registers itself on first use.
    assert_ne!(
        unsafe { g_type_is_a(gtype, camel_transport_get_type()) },
        GFALSE,
        "the transport does not derive from CamelTransport"
    );
}

/// The name follows `Camel<Protocol>Transport`, like every transport in
/// Camel's own tree. A GObject warning about the wrong service type shows this
/// name to the user. `g_type_from_name` looks up the type by this name.
#[test]
fn the_transport_is_named_the_way_camels_own_transports_are() {
    assert_eq!(
        <JmapTransport as ObjectSubclass>::NAME,
        c"CamelJmapTransport"
    );

    let gtype = transport_type();
    // SAFETY: NAME is a 'static, NUL-terminated string.
    assert_eq!(gtype, unsafe {
        g_type_from_name(<JmapTransport as ObjectSubclass>::NAME.as_ptr())
    });
}

/// The transport uses the same settings class as the store, because both
/// belong to the same account. A transport that inherited `CamelSettings`
/// would have no host, port, or user field. `e_source_camel_configure_service`
/// would then have nowhere to write the server the user typed.
#[test]
fn the_transport_is_configured_through_the_accounts_settings_class() {
    // SAFETY: a live GType of a class that has a settings_type; the code
    // returns the reference.
    unsafe {
        let class = g_type_class_ref(transport_type()).cast::<CamelServiceClass>();
        assert_eq!(
            (*class).settings_type,
            settings_type(),
            "the transport does not name the JMAP settings class"
        );
        g_type_class_unref(class.cast());
    }
}

// ---------------------------------------------------------------------------
// its connection

/// A transport that Camel constructed but has not yet connected holds no
/// connection, and reports that correctly. `connect_sync` reads this state to
/// short-circuit.
#[test]
fn a_new_transport_holds_no_connection() {
    let transport = JmapTransport::detached();
    assert!(!transport.is_connected());
}

/// This test calls the same `authenticate` function that the store's
/// `authenticate_sync` calls. Opening an account is the same operation for
/// either service.
#[test]
fn authenticating_a_transport_opens_a_connection() {
    let server = seeded();
    let transport = JmapTransport::detached();

    authenticate(&*transport, &config(&server), Credentials::none()).expect("authenticated");

    assert!(transport.is_connected());
}

/// This is the purpose of the type. Evolution connects the two services of an
/// account independently: the store when the user opens a folder, the
/// transport when the user sends a message. Neither service can reach the
/// other's connection. A transport with no connection of its own would have
/// nothing to send over.
#[test]
fn the_transports_connection_is_its_own_and_not_the_stores() {
    let server = seeded();
    let store = JmapStore::detached();
    let transport = JmapTransport::detached();

    authenticate(&*store, &config(&server), Credentials::none()).expect("the store authenticated");
    assert!(
        !transport.is_connected(),
        "connecting the store connected the transport"
    );

    authenticate(&*transport, &config(&server), Credentials::none())
        .expect("the transport authenticated");
    assert!(store.is_connected() && transport.is_connected());

    // The reverse also holds. Camel disconnects a store on its own schedule,
    // for example when an account goes offline or a folder tree sits idle.
    // A send is not part of that schedule.
    assert!(store.drop_connection());
    assert!(
        transport.is_connected(),
        "disconnecting the store took the transport's connection with it"
    );
}

/// What `disconnect_sync` does on this service, including a service that
/// never connected. Camel asks this of every service on shutdown.
#[test]
fn disconnecting_drops_whatever_the_authentication_installed() {
    let server = seeded();
    let transport = JmapTransport::detached();
    assert!(!transport.drop_connection());

    authenticate(&*transport, &config(&server), Credentials::none()).expect("authenticated");
    assert!(transport.drop_connection());
    assert!(!transport.is_connected());
}

/// The store's rule applies to the transport too. Camel re-authenticates a
/// service that already has a connection, and a failed attempt must not break
/// the working connection. A user whose password prompt is still open should
/// not lose the ability to send the message they were about to send.
#[test]
fn a_failed_attempt_leaves_a_working_connection_alone() {
    let server = seeded();
    let transport = JmapTransport::detached();
    authenticate(&*transport, &config(&server), Credentials::none()).expect("authenticated");

    let unreachable = ServerConfig {
        // Port 1 is reserved and nothing listens there.
        target: ConnectTarget::Origin("http://127.0.0.1:1".to_owned()),
        user: None,
    };
    authenticate(&*transport, &unreachable, Credentials::none())
        .expect_err("nothing listens there");

    assert!(transport.is_connected());
}

// ---------------------------------------------------------------------------
// what Camel calls it

/// The `get_name` slot, on a transport that Camel itself constructed. The
/// store gives the same answer, because there is one account, one server, and
/// one name. This test checks only that the slot is filled.
/// `camel_service_get_name` contains `g_return_val_if_fail (class->get_name !=
/// NULL, NULL)`. A transport with no `get_name` slot would return NULL and log
/// a critical message every time Camel described a send.
#[test]
fn the_transport_names_the_account_it_sends_through() {
    let account = Account::open();
    let transport = Transport::open(&account);
    configure(&transport, "jmap.example.com", 8443, Some("vera"));

    assert_eq!(
        name_of(&transport, true).as_deref(),
        Some("JMAP server jmap.example.com")
    );
    assert_eq!(
        name_of(&transport, false).as_deref(),
        Some("JMAP service for vera on jmap.example.com:8443")
    );
}

/// Writes a server onto the transport's settings, the way EDS configures one.
fn configure(transport: &Transport, host: &str, port: u16, user: Option<&str>) {
    let host = std::ffi::CString::new(host).expect("a host with no NUL in it");
    let user = user.map(|user| std::ffi::CString::new(user).expect("a user with no NUL in it"));
    // SAFETY: a live `CamelService`. Its settings are a `CamelJmapSettings`,
    // which is also a `CamelNetworkSettings`. The setters copy the strings.
    // The code returns the reference.
    unsafe {
        let settings = camel_service_ref_settings(transport.service);
        assert!(!settings.is_null(), "the transport has no settings");
        let network = settings.cast::<CamelNetworkSettings>();
        camel_network_settings_set_host(network, host.as_ptr());
        camel_network_settings_set_port(network, port);
        camel_network_settings_set_user(
            network,
            user.as_ref().map_or(ptr::null(), |user| user.as_ptr()),
        );
        gobject_sys::g_object_unref(settings.cast());
    }
}

/// What `camel_service_get_name` answers, as a Rust string. Camel returns NULL
/// for a class that left the slot empty; this function returns `None` for
/// that case.
fn name_of(transport: &Transport, brief: bool) -> Option<String> {
    // SAFETY: a live `CamelService`. The returned string is a GLib allocation
    // that this call owns and frees.
    unsafe {
        let name = camel_service_get_name(
            transport.service.cast::<CamelService>(),
            if brief { GTRUE } else { GFALSE },
        );
        if name.is_null() {
            return None;
        }
        let owned = CStr::from_ptr(name).to_string_lossy().into_owned();
        glib_sys::g_free(name.cast());
        Some(owned)
    }
}
