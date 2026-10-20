// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests for the `fan_out` layer of this crate against a real JMAP server.
//
// `jmap-collection-sync/tests/live_server.rs` tests the round trip of
// `create_collection`, `delete_collection` and `Fanout::discover` through real
// Stalwart. That file cannot test the layer of this crate:
// `fan_out`/`apply_fanout`/`adopt` in `fan_out.rs`. This layer changes a
// discovered `Fanout` into created, written and exported `ESource` children.
// `tests/fan_out.rs` tests the layer only against `jmap-mockd`. This file
// tests it against the resources, names and `myRights` of a real server. It
// uses the fake `Collection` trait implementation of `tests/fan_out.rs`, because
// this runner has no `evolution-source-registry` session bus.
//
// The test is skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER` or
// `_PASSWORD` is not set. See docs/manual-test-live-server.md.

use std::cell::RefCell;
use std::env;
use std::ffi::CString;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, ESource, ESourceAuthentication,
    e_source_authentication_get_host, e_source_authentication_get_port,
    e_source_authentication_get_type, e_source_get_display_name, e_source_get_extension,
    e_source_has_extension, e_source_new_with_uid,
};
use glib_sys::GFALSE;
use gobject_sys::{g_object_ref, g_object_unref};
use jmap_backend_collection::authenticate::Login;
use jmap_backend_collection::collection_source::Server;
use jmap_backend_collection::fan_out::fan_out;
use jmap_backend_collection::resource_id::resource_id_of;
use jmap_backend_core::source::ConnectTarget;
use jmap_client::{Client, Credentials};
use jmap_collection_sync::child_source::Connection;
use jmap_collection_sync::{
    ChildKind, Doomed, Parts, Requested, create_collection, delete_collection,
};

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Mirrors `jmap-collection-sync/tests/live_server.rs::connect_for_write`.
fn connect_for_write() -> Option<Client> {
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").ok()?;
    let password = env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD")
        .expect("JMAP_LIVE_SERVER_WRITE_USER is set but JMAP_LIVE_SERVER_WRITE_PASSWORD is not");
    let origin = env::var("JMAP_LIVE_SERVER_URL")
        .expect("set JMAP_LIVE_SERVER_URL alongside JMAP_LIVE_SERVER_WRITE_USER");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");

    let client = Client::builder()
        .rebase_urls_to_origin(rebase)
        .connect(&origin, Credentials::basic(user, password))
        .expect("could not fetch the session document for the write-test account");
    Some(client)
}

/// A live `ESource` that this test holds one reference to.
struct Source(*mut ESource);

impl Source {
    fn dup(&self) -> *mut ESource {
        // SAFETY: a live GObject this test holds a reference to.
        unsafe { g_object_ref(self.0.cast()) }.cast()
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: this struct holds the last reference. The call under test
        // consumed each reference that this fixture gave out.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// The `ECollectionBackend` half of a fan-out, without an `ECollectionBackend`.
/// It has the same shape as the fixture of `tests/fan_out.rs`, without the
/// parts that this file does not need (a cache of earlier sessions and a
/// refusal list).
#[derive(Default)]
struct Collection {
    created: RefCell<Vec<(String, Source)>>,
    published: RefCell<Vec<*mut ESource>>,
    remote_deletable: RefCell<Vec<(String, bool)>>,
}

impl Collection {
    fn published_ids(&self) -> Vec<String> {
        self.published
            .borrow()
            .iter()
            .map(|source| {
                // SAFETY: this collection still holds a reference to each
                // published pointer.
                unsafe { resource_id_of(*source) }
                    .expect("a child was exported with no resource id on it")
            })
            .collect()
    }

    fn child(&self, resource_id: &str) -> *mut ESource {
        self.created
            .borrow()
            .iter()
            .find(|(id, _)| id == resource_id)
            .map(|(_, source)| source.0)
            .unwrap_or_else(|| panic!("no child was created for {resource_id}"))
    }

    fn remote_deletable_of(&self, resource_id: &str) -> Option<bool> {
        self.remote_deletable
            .borrow()
            .iter()
            .rev()
            .find(|(id, _)| id == resource_id)
            .map(|(_, deletable)| *deletable)
    }
}

// SAFETY: each returned pointer is a valid `ESource` with its own reference.
// `g_object_ref` takes that reference immediately before the return.
unsafe impl jmap_backend_collection::fan_out::Collection for Collection {
    fn new_child(&self, resource_id: &str) -> *mut ESource {
        let uid = CString::new(format!("jmap-{resource_id}")).expect("no NUL in a resource id");
        let mut error = ptr::null_mut();
        // SAFETY: the documented arguments are a NUL-terminated uid, the
        // default main context and a GError out-parameter.
        let raw = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!raw.is_null(), "e_source_new_with_uid failed");
        let source = Source(raw);
        let handed_out = source.dup();
        self.created
            .borrow_mut()
            .push((resource_id.to_owned(), source));
        handed_out
    }

    fn is_new_child(&self, _child: *mut ESource) -> bool {
        // Each child in this fan-out is new, because the test puts no cache of
        // an earlier session in place.
        true
    }

    fn publish(&self, child: *mut ESource) {
        self.published.borrow_mut().push(child);
    }

    fn existing_children(&self) -> Vec<*mut ESource> {
        // This fixture has no existing children, so a fan-out here removes
        // nothing. `tests/fan_out.rs` tests the removal decision against the
        // mock. The "EDS refuses" branch does not need a real server.
        Vec::new()
    }

    fn set_remote_deletable(&self, child: *mut ESource, deletable: bool) {
        // SAFETY: each source here came from `new_child` and is alive for this
        // call.
        let resource_id = unsafe { resource_id_of(child) }.unwrap_or_default();
        self.remote_deletable
            .borrow_mut()
            .push((resource_id, deletable));
    }
}

/// The server address of the login of this test. It is intentionally not the
/// address of the real server. `fan_out` must write children from the
/// connection of the login, not from the address that discovery used. Only a
/// different address can show the difference.
fn connection() -> Connection {
    Connection {
        host: "jmap.example.com".to_owned(),
        port: Some(8443),
        user: Some("agent1@agent-backendcollection.test".to_owned()),
        auth_method: Some("plain/password".to_owned()),
        secure: true,
    }
}

fn host_and_port(source: *mut ESource) -> (Option<String>, u16) {
    // SAFETY: the function takes no arguments, and the type system
    // initialises itself.
    unsafe { e_source_authentication_get_type() };
    assert!(
        // SAFETY: a live source, and a header constant.
        unsafe { e_source_has_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()) }
            != GFALSE,
        "a child with no [Authentication] reaches no server"
    );
    // SAFETY: the extension is present, and the source owns it.
    unsafe {
        let auth: *mut ESourceAuthentication =
            e_source_get_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast();
        (
            jmap_backend_core::marshal::read_string(e_source_authentication_get_host(auth)),
            e_source_authentication_get_port(auth),
        )
    }
}

fn display_name(source: *mut ESource) -> Option<String> {
    // SAFETY: a live source. The source owns the returned name.
    unsafe { jmap_backend_core::marshal::read_string(e_source_get_display_name(source)) }
}

/// Creates a real address book and a real calendar, then runs a real
/// `fan_out` against their account. For each collection, the test makes sure
/// of these points:
/// - The fan-out adopts it.
/// - The child uses the connection of the login, not the server address.
/// - The child is exported under the resource id from `create_collection`.
/// - The `myRights` of a new discovery give `set_remote_deletable` the same
///   value as the fallback of `create_collection`.
///
/// Then the test deletes both collections.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn fan_out_adopts_and_writes_real_server_collections() {
    let Some(write_client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };

    let suffix = unique_suffix();
    let book_name = format!("agent-fanout-book-{suffix}");
    let cal_name = format!("agent-fanout-cal-{suffix}");

    let created_book = create_collection(
        &write_client,
        &Requested {
            kind: ChildKind::AddressBook,
            display_name: book_name.clone(),
        },
    )
    .expect("AddressBook/set create failed against the real server");
    let created_cal = create_collection(
        &write_client,
        &Requested {
            kind: ChildKind::Calendar,
            display_name: cal_name.clone(),
        },
    )
    .expect("Calendar/set create failed against the real server");

    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").expect("connect_for_write succeeded");
    let password =
        env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD").expect("connect_for_write succeeded");
    let origin = env::var("JMAP_LIVE_SERVER_URL").expect("connect_for_write succeeded");
    let login = Login {
        server: Server {
            target: ConnectTarget::Origin(origin),
            connection: connection(),
            rebase_urls: env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|v| v != "0"),
        },
        parts: Parts::ALL,
        credentials: Credentials::basic(user, password),
    };

    let collection = Collection::default();
    // SAFETY: the `Collection` implementation above obeys the contract of the
    // trait.
    let report =
        unsafe { fan_out(&collection, &login) }.expect("fan_out failed against the real server");

    for (created, name) in [(&created_book, &book_name), (&created_cal, &cal_name)] {
        assert!(
            report.children.contains(&created.resource_id),
            "the real {:?} just created should be adopted by the fan-out: {report:?}",
            created.kind
        );
        assert!(
            collection.published_ids().contains(&created.resource_id),
            "a newly created child must be exported, not left unpublished"
        );

        let source = collection.child(&created.resource_id);
        let written_connection = connection();
        assert_eq!(
            host_and_port(source),
            (
                Some(written_connection.host),
                written_connection.port.unwrap()
            ),
            "a child must be written from the login's own connection, \
             not the address discovery actually reached"
        );
        assert_eq!(
            display_name(source).as_deref(),
            Some(name.as_str()),
            "the sidebar row should be named the way the real server named the collection"
        );
        assert_eq!(
            collection.remote_deletable_of(&created.resource_id),
            Some(created.remote_deletable),
            "a fresh discovery's own myRights should reach set_remote_deletable \
             the same way create_collection's own fallback already answered it"
        );
    }

    delete_collection(
        &write_client,
        &Doomed {
            kind: ChildKind::AddressBook,
            collection_id: created_book.collection_id,
        },
    )
    .expect("AddressBook/set destroy failed against the real server");
    delete_collection(
        &write_client,
        &Doomed {
            kind: ChildKind::Calendar,
            collection_id: created_cal.collection_id,
        },
    )
    .expect("Calendar/set destroy failed against the real server");
}
