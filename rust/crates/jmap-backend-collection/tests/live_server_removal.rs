// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// `fan_out`'s removal half against a real JMAP server.
//
// `tests/live_server.rs` proves the *adopt* half of `fan_out` against real
// Stalwart, but its own `Collection` fixture's `existing_children` always
// answers empty, by its own comment deferring the removal decision to
// `tests/fan_out.rs`'s mock. `tests/removal.rs` in turn only ever proves the
// decision (`Fanout::is_obsolete`) against a hand-built `Fanout`, and the
// mechanics (`e_source_remove_sync`) only down the "EDS refuses" branch —
// these synthetic sources carry no D-Bus object, so a *successful* removal is
// not reachable without a real `evolution-source-registry` session, same
// caveat `docs/manual-test-live-server.md` already carries for other wiring.
//
// What neither file reaches is the join: does a real server-side deletion,
// seen through a second real `Fanout::discover`, actually drive `fan_out` to
// judge the previously-adopted child obsolete and attempt its removal? This
// is that confirmation. The removal attempt itself is still expected to be
// refused here for the registry-less reason above; what this test pins is
// that it is attempted, and attempted against the right resource id.
//
// Skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` are
// unset; see docs/manual-test-live-server.md.

use std::cell::RefCell;
use std::env;
use std::ffi::CString;
use std::ptr;

use eds_sys::{ESource, e_source_new_with_uid};
use gobject_sys::{g_object_ref, g_object_unref};
use jmap_backend_collection::authenticate::Login;
use jmap_backend_collection::collection_source::Server;
use jmap_backend_collection::fan_out::fan_out;
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

/// Mirrors `tests/live_server.rs::connect_for_write`.
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
        // consumed by the call under test, or is released by `existing_children`'s
        // own caller (`apply_fanout`) the same way.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// Same fixture shape as `tests/live_server.rs`'s `Collection`, plus the one
/// thing that file's does not need: a seedable `existing` list, so a second
/// `fan_out` pass can be handed back the child the first pass created.
#[derive(Default)]
struct Collection {
    created: RefCell<Vec<(String, Source)>>,
    published: RefCell<Vec<*mut ESource>>,
    existing: RefCell<Vec<Source>>,
}

impl Collection {
    fn child(&self, resource_id: &str) -> *mut ESource {
        self.created
            .borrow()
            .iter()
            .find(|(id, _)| id == resource_id)
            .map(|(_, source)| source.0)
            .unwrap_or_else(|| panic!("no child was created for {resource_id}"))
    }

    /// Hands `source` to the next `existing_children()` call, as a real
    /// `ECollectionBackend` would after a first populate wrote it.
    fn seed_existing(&self, source: *mut ESource) {
        // SAFETY: the caller holds a live reference to `source` for the
        // duration of this call; this takes its own.
        let owned = unsafe { g_object_ref(source.cast()) }.cast();
        self.existing.borrow_mut().push(Source(owned));
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
        self.existing.borrow().iter().map(Source::dup).collect()
    }

    fn set_remote_deletable(&self, _child: *mut ESource, _deletable: bool) {}
}

/// Mirrors `tests/live_server.rs`'s own `connection()`.
fn connection() -> Connection {
    Connection {
        host: "jmap.example.com".to_owned(),
        port: Some(8443),
        user: Some("agent1@agent-backendremoval.test".to_owned()),
        auth_method: Some("plain/password".to_owned()),
        secure: true,
    }
}

/// Creates a real address book, runs a real `fan_out` to adopt it, deletes it
/// on the real server, then runs `fan_out` a second time with the first
/// pass's own child seeded as an existing one. Confirms the real post-deletion
/// `Fanout::discover` drives `is_obsolete` to pick exactly that child for
/// removal, not that removal itself succeeds (see the module comment on why
/// that half needs a real registry session this runner has none of).
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_collection_deleted_on_the_server_is_judged_obsolete_on_the_next_fan_out() {
    let Some(write_client) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };

    let suffix = unique_suffix();
    let book_name = format!("agent-fanout-removal-{suffix}");

    let created_book = create_collection(
        &write_client,
        &Requested {
            kind: ChildKind::AddressBook,
            display_name: book_name.clone(),
        },
    )
    .expect("AddressBook/set create failed against the real server");

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
    let first_pass = unsafe { fan_out(&collection, &login) }
        .expect("first fan_out failed against the real server");
    assert!(
        first_pass.children.contains(&created_book.resource_id),
        "the newly created book should be adopted on the first pass: {first_pass:?}"
    );

    collection.seed_existing(collection.child(&created_book.resource_id));

    delete_collection(
        &write_client,
        &Doomed {
            kind: ChildKind::AddressBook,
            collection_id: created_book.collection_id,
        },
    )
    .expect("AddressBook/set destroy failed against the real server");

    // SAFETY: as above.
    let second_pass = unsafe { fan_out(&collection, &login) }
        .expect("second fan_out failed against the real server");

    assert!(
        !second_pass.children.contains(&created_book.resource_id),
        "a collection deleted on the server must not be re-adopted: {second_pass:?}"
    );
    assert!(
        second_pass
            .not_removed
            .iter()
            .any(|not_removed| not_removed.resource_id == created_book.resource_id),
        "the real post-deletion discovery should judge the child obsolete \
         and attempt its removal, even though the removal itself is refused \
         here for the usual no-registry reason: {second_pass:?}"
    );
}
