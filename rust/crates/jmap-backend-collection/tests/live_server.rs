// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// `fan_out`'s own layer against a real JMAP server.
//
// `jmap-collection-sync/tests/live_server.rs` already proves
// `create_collection`/`delete_collection` and `Fanout::discover` round-trip
// through real Stalwart. What that file cannot reach is this crate's own
// layer on top: `fan_out`/`apply_fanout`/`adopt` in `fan_out.rs`, which turn a
// discovered `Fanout` into created, written and exported `ESource` children.
// Every test of that layer so far (`tests/fan_out.rs`) has run against
// `jmap-mockd`, never against a real server's own resources, names and
// `myRights`. This is that confirmation, using the same fake `Collection`
// trait double `tests/fan_out.rs` uses in place of a live
// `evolution-source-registry` session bus this runner has none of.
//
// Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
// unset; see docs/manual-test-live-server.md.

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

/// A live `ESource` this test holds one reference to.
struct Source(*mut ESource);

impl Source {
    fn dup(&self) -> *mut ESource {
        // SAFETY: a live GObject this test holds a reference to.
        unsafe { g_object_ref(self.0.cast()) }.cast()
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: this holds the last reference; every one handed out was
        // consumed by the call under test.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// The `ECollectionBackend` half of a fan-out, without one — same shape as
/// `tests/fan_out.rs`'s fixture, minus the pieces this file does not need
/// (a cache of previous sessions, a refusal list).
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
                // SAFETY: every published pointer is one this collection
                // still holds a reference to.
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

// SAFETY: every pointer handed out is a valid `ESource` carrying a reference
// of its own, taken with `g_object_ref` just before it is returned.
unsafe impl jmap_backend_collection::fan_out::Collection for Collection {
    fn new_child(&self, resource_id: &str) -> *mut ESource {
        let uid = CString::new(format!("jmap-{resource_id}")).expect("no NUL in a resource id");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a
        // GError out-parameter are the documented arguments.
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
        // Every child this test's fan-out sees is newly created: it seeds no
        // cache of a previous session.
        true
    }

    fn publish(&self, child: *mut ESource) {
        self.published.borrow_mut().push(child);
    }

    fn existing_children(&self) -> Vec<*mut ESource> {
        // This fixture never seeds an existing child, so a fan-out here never
        // has anything to remove — the removal decision is `tests/fan_out.rs`'s
        // to cover, against the mock, where the "EDS refuses" branch it hits
        // either way is exercisable without a real server at all.
        Vec::new()
    }

    fn set_remote_deletable(&self, child: *mut ESource, deletable: bool) {
        // SAFETY: every source this is called on is one `new_child` handed
        // back, alive for this call.
        let resource_id = unsafe { resource_id_of(child) }.unwrap_or_default();
        self.remote_deletable
            .borrow_mut()
            .push((resource_id, deletable));
    }
}

/// Where this test's login says its server is — arbitrary and unrelated to
/// the real server's own address, on purpose: `fan_out` must write children
/// from the login's own connection, not from wherever discovery actually
/// reached, and only a connection that disagrees with the real origin can
/// tell those two apart.
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
    // SAFETY: no arguments, and the type system initialises itself.
    unsafe { e_source_authentication_get_type() };
    assert!(
        // SAFETY: a live source, and a header constant.
        unsafe { e_source_has_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()) }
            != GFALSE,
        "a child with no [Authentication] reaches no server"
    );
    // SAFETY: the extension is present and owned by the source.
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
    // SAFETY: a live source; the name it answers with is its own.
    unsafe { jmap_backend_core::marshal::read_string(e_source_get_display_name(source)) }
}

/// Creates a real address book and a real calendar, runs a real `fan_out`
/// against the account that holds them, confirms each is adopted, written
/// from the login's own connection (not the server's address) and exported
/// under the resource id `create_collection` already named, and that a fresh
/// discovery's own `myRights` reaches `set_remote_deletable` the same way
/// `create_collection`'s fallback answered it — then deletes both.
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
    // SAFETY: `Collection`'s impl above satisfies the trait's contract.
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
