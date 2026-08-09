// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `CamelJmapTransport`: the second service a JMAP account has, and the one
//! that never reads a folder.
//!
//! Camel gives an account two services and no way for either to reach the
//! other: a `CamelStore` for the mail it holds and a `CamelTransport` for the
//! mail it sends. Evolution constructs them from two `ESource`s, hands each its
//! own settings object, and connects each on its own. So a JMAP transport is
//! not a view of the store — it is a service of its own, with a connection of
//! its own to the same server, and these tests are about exactly that: that it
//! is a `CamelTransport`, that it is configured through the account's settings
//! class rather than through a stock one with no host in it, that authenticating
//! it opens a connection, and that the connection is *its* and not the store's.
//!
//! What is deliberately not here is `send_to_sync`. The vfunc is a later
//! increment, and until it exists the provider's transport slot stays
//! `G_TYPE_INVALID` — see `tests/provider.rs`, which pins that.

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
use jmap_backend_core::subclass::ObjectSubclass;
use jmap_mail::server::ServerConfig;
use jmap_mail::service::authenticate;
use jmap_mail::settings::settings_type;
use jmap_mail::store::JmapStore;
use jmap_mail::transport::{JmapTransport, transport_type};
use jmap_mock::MockServer;
use jmap_proto::mail::role;

fn config(server: &MockServer) -> ServerConfig {
    ServerConfig {
        origin: server.origin().to_owned(),
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

/// The parent is the whole reason this type exists: `send_to_sync` is declared
/// on `CamelTransportClass` and nowhere else, and `camel_session_add_service`
/// refuses a type that is not one when Evolution asks it for the sending half
/// of an account.
#[test]
fn the_transport_is_a_camel_transport() {
    let gtype = transport_type();
    assert_ne!(gtype, G_TYPE_INVALID, "the transport type did not register");
    assert_eq!(
        gtype,
        transport_type(),
        "registering it twice made two types"
    );

    // SAFETY: a live GType, and a type that registers itself on first use.
    assert_ne!(
        unsafe { g_type_is_a(gtype, camel_transport_get_type()) },
        GFALSE,
        "the transport does not derive from CamelTransport"
    );
}

/// `Camel<Protocol>Transport`, like every transport in Camel's own tree: the
/// type name is what a user sees in a GObject warning about the wrong service
/// type, and it is what `g_type_from_name` finds the type by.
#[test]
fn the_transport_is_named_the_way_camels_own_transports_are() {
    assert_eq!(
        <JmapTransport as ObjectSubclass>::NAME,
        c"CamelJmapTransport"
    );

    let gtype = transport_type();
    // SAFETY: NAME is a 'static NUL-terminated string.
    assert_eq!(gtype, unsafe {
        g_type_from_name(<JmapTransport as ObjectSubclass>::NAME.as_ptr())
    });
}

/// The same settings class as the store's, because it is the same account: a
/// transport that inherited `CamelSettings` would have no host, no port and no
/// user, and `e_source_camel_configure_service` would have nowhere to write the
/// server the user typed.
#[test]
fn the_transport_is_configured_through_the_accounts_settings_class() {
    // SAFETY: a live GType of a class that has one; the reference is given back.
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

/// A transport Camel has constructed and not yet connected holds nothing, and
/// says so — which is what `connect_sync`'s short-circuit reads.
#[test]
fn a_new_transport_holds_no_connection() {
    let transport = JmapTransport::detached();
    assert!(!transport.is_connected());
}

/// The same `authenticate` the store's `authenticate_sync` calls, on the other
/// service: one function, because opening an account is the same operation
/// whichever half of it asked.
#[test]
fn authenticating_a_transport_opens_a_connection() {
    let server = seeded();
    let transport = JmapTransport::detached();

    authenticate(&*transport, &config(&server), None).expect("authenticated");

    assert!(transport.is_connected());
}

/// The point of the type. Evolution connects the two services of an account
/// independently — the store when the user opens a folder, the transport when
/// they press Send — and neither can reach the other's connection, so a
/// transport that had none of its own would have nothing to submit over.
#[test]
fn the_transports_connection_is_its_own_and_not_the_stores() {
    let server = seeded();
    let store = JmapStore::detached();
    let transport = JmapTransport::detached();

    authenticate(&*store, &config(&server), None).expect("the store authenticated");
    assert!(
        !transport.is_connected(),
        "connecting the store connected the transport"
    );

    authenticate(&*transport, &config(&server), None).expect("the transport authenticated");
    assert!(store.is_connected() && transport.is_connected());

    // And the other way round: Camel disconnects a store on its own schedule —
    // an account going offline, a folder tree that has been idle — and a send
    // is not part of that.
    assert!(store.drop_connection());
    assert!(
        transport.is_connected(),
        "disconnecting the store took the transport's connection with it"
    );
}

/// What `disconnect_sync` amounts to on this service, including on one that
/// never connected — which is what Camel asks of every service on shutdown.
#[test]
fn disconnecting_drops_whatever_the_authentication_installed() {
    let server = seeded();
    let transport = JmapTransport::detached();
    assert!(!transport.drop_connection());

    authenticate(&*transport, &config(&server), None).expect("authenticated");
    assert!(transport.drop_connection());
    assert!(!transport.is_connected());
}

/// The store's rule, on the transport: Camel re-authenticates a service it
/// already has a connection for, and an attempt that fails must not take the
/// working one with it — a user whose password prompt is still open should not
/// find that the message they were about to send now has nothing to go out
/// over.
#[test]
fn a_failed_attempt_leaves_a_working_connection_alone() {
    let server = seeded();
    let transport = JmapTransport::detached();
    authenticate(&*transport, &config(&server), None).expect("authenticated");

    let unreachable = ServerConfig {
        // Port 1 is reserved and nothing listens there.
        origin: "http://127.0.0.1:1".to_owned(),
        user: None,
    };
    authenticate(&*transport, &unreachable, None).expect_err("nothing listens there");

    assert!(transport.is_connected());
}

// ---------------------------------------------------------------------------
// what Camel calls it

/// The `get_name` slot, on a transport Camel itself constructed. It is the same
/// answer the store gives — one account, one server, one name — and the test is
/// that the slot is filled at all: `camel_service_get_name` is a
/// `g_return_val_if_fail (class->get_name != NULL, NULL)`, so a transport that
/// installed none would answer NULL and log a critical every time Camel wrote a
/// sentence about a send.
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
    // SAFETY: a live `CamelService`, whose settings are a `CamelJmapSettings`
    // and therefore a `CamelNetworkSettings`; the strings are copied by the
    // setters and the reference is given back.
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

/// What `camel_service_get_name` answers, as a Rust string; NULL — which is
/// what Camel returns for a class that filled none of the slot in — comes back
/// as `None`.
fn name_of(transport: &Transport, brief: bool) -> Option<String> {
    // SAFETY: a live `CamelService`; the string returned is a GLib allocation
    // this call owns and frees.
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
