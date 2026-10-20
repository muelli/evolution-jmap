// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests for the removal half of `fan_out` against a real JMAP server.
//
// `tests/live_server.rs` tests the *adopt* half of `fan_out` against real
// Stalwart. Its `Collection` fixture always returns no `existing_children`. It
// leaves the removal decision to `tests/fan_out.rs` and the mock.
// `tests/removal.rs` tests the decision (`Fanout::is_obsolete`) only with a
// `Fanout` made by hand. It tests the removal (`e_source_remove_sync`) only on
// the "EDS refuses" branch. These synthetic sources have no D-Bus object. Thus a
// *successful* removal needs a real `evolution-source-registry` session.
// `docs/manual-test-live-server.md` gives the same limit for other wiring.
//
// Neither file tests the join. This test makes sure that a real deletion on
// the server, seen through a second real `Fanout::discover`, causes `fan_out`
// to find the adopted child obsolete and to try its removal. EDS still refuses
// the removal here, because there is no registry. The test makes sure that
// `fan_out` tries the removal for the correct resource id.
//
// The test is skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER` or
// `_PASSWORD` is not set. See docs/manual-test-live-server.md.

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
        // consumed each reference that this fixture gave out. For
        // `existing_children`, its caller (`apply_fanout`) releases them.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// The same fixture shape as `Collection` in `tests/live_server.rs`, with one
/// addition: an `existing` list that the test can fill. Thus a second `fan_out`
/// pass gets the child that the first pass created.
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

    /// Gives `source` to the next `existing_children()` call, as a real
    /// `ECollectionBackend` does after a first populate wrote it.
    fn seed_existing(&self, source: *mut ESource) {
        // SAFETY: the caller holds a live reference to `source` during this
        // call. This function takes its own reference.
        let owned = unsafe { g_object_ref(source.cast()) }.cast();
        self.existing.borrow_mut().push(Source(owned));
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
        self.existing.borrow().iter().map(Source::dup).collect()
    }

    fn set_remote_deletable(&self, _child: *mut ESource, _deletable: bool) {}
}

/// Mirrors `connection()` in `tests/live_server.rs`.
fn connection() -> Connection {
    Connection {
        host: "jmap.example.com".to_owned(),
        port: Some(8443),
        user: Some("agent1@agent-backendremoval.test".to_owned()),
        auth_method: Some("plain/password".to_owned()),
        secure: true,
    }
}

/// Creates a real address book and runs a real `fan_out` to adopt it. Then the
/// test deletes it on the real server and runs `fan_out` again, with the child
/// of the first pass as an existing child. The test makes sure that
/// `is_obsolete` selects exactly that child for removal after the deletion. It
/// does not make sure that the removal succeeds, because that needs a real
/// registry session (see the module comment).
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
    // SAFETY: the `Collection` implementation above obeys the contract of the
    // trait.
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
