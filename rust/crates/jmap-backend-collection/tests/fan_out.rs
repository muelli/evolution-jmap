// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The body of the fan-out: what one authenticated login does to a collection's
// children, against real `ESource`s and a real `jmap-mockd`.
//
// The three `ECollectionBackend` methods the fan-out needs are a trait, so the
// collection here is a stand-in — but nothing else is. The sources it hands out
// are made by `e_source_new_with_uid`, which is what EDS itself uses for a
// source read from a keyfile; the settings are written by the same `apply` the
// real backend calls; the resource ids are read back by the same `resource_id_of`
// the `dup_resource_id` vfunc answers with; and the collections come off a
// running mock server. What is stubbed is the part that needs a session bus, and
// only that part.

use std::cell::RefCell;
use std::ffi::CString;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, ESource, ESourceAuthentication,
    e_source_authentication_get_host, e_source_authentication_get_port,
    e_source_authentication_get_type, e_source_get_display_name, e_source_get_extension,
    e_source_has_extension, e_source_new_with_uid,
};
use glib_sys::GFALSE;
use gobject_sys::{GObject, g_object_ref, g_object_unref};
use jmap_backend_collection::authenticate::Login;
use jmap_backend_collection::collection_source::Server;
use jmap_backend_collection::fan_out::{Adopted, Populated, adopt, apply_fanout, fan_out};
use jmap_backend_collection::resource_id::resource_id_of;
use jmap_backend_core::source::ConnectTarget;
use jmap_client::Credentials;
use jmap_collection_sync::child_source::{Connection, EXTENSION_AUTHENTICATION};
use jmap_collection_sync::{
    ChildKind, CollectionLayout, Fanout, Parts, Resource, ServiceAccount, Setting,
};
use jmap_mock::{AccountState, DEFAULT_ACCOUNT_ID, MockServer, ServerState};
use jmap_proto::Id;

mod common;
use common::{with_timeout, with_timeout_duration};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A live `ESource` this test holds one reference to, as `e_source_new_with_uid`
/// hands one back.
struct Source(*mut ESource);

impl Source {
    fn new(uid: &str) -> Self {
        let uid = CString::new(uid).expect("no NUL in a test uid");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a GError
        // out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    /// A reference of its own, which is what both `ECollectionBackend` getters
    /// behind the trait hand out and what the fan-out consumes.
    fn dup(&self) -> *mut ESource {
        // SAFETY: a live GObject this test holds a reference to.
        unsafe { g_object_ref(self.0.cast()) }.cast()
    }

    /// The GObject reference count, which is how a reference `adopt` took but
    /// did not give back — or gave back twice — becomes an assertion rather
    /// than a leak or a use-after-free nobody notices.
    fn ref_count(&self) -> u32 {
        // SAFETY: a live GObject; `ref_count` is a field of the instance struct.
        unsafe { (*self.0.cast::<GObject>()).ref_count }
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: this holds the last reference; every one handed out was
        // consumed by the call under test.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// The `ECollectionBackend` half of a fan-out, without one.
#[derive(Default)]
struct Collection {
    /// The children this collection already has, in the order
    /// `e_collection_backend_list_*_sources` would answer.
    existing: Vec<Source>,
    /// Resource ids `new_child` draws from the cache of previous sessions
    /// instead of creating: children that already exist and are therefore not
    /// new, so `populate` has already exported them.
    cached: Vec<(String, Source)>,
    /// Resource ids `new_child` answers NULL for.
    refused: Vec<String>,
    /// Every child created, in order.
    created: RefCell<Vec<(String, Source)>>,
    /// Every child handed to `e_source_registry_server_add_source`.
    published: RefCell<Vec<*mut ESource>>,
    /// Which trait method was called when, so that an ordering decision is a
    /// test rather than a comment.
    calls: RefCell<Vec<&'static str>>,
}

impl Collection {
    fn refusing(resource_id: &str) -> Self {
        Self {
            refused: vec![resource_id.to_owned()],
            ..Self::default()
        }
    }

    /// The resource ids of the children it created, in order.
    fn created_ids(&self) -> Vec<String> {
        self.created
            .borrow()
            .iter()
            .map(|(resource_id, _)| resource_id.clone())
            .collect()
    }

    /// The resource ids of the children it exported.
    fn published_ids(&self) -> Vec<String> {
        self.published
            .borrow()
            .iter()
            .map(|source| {
                // SAFETY: every published pointer is one this collection still
                // holds a reference to.
                unsafe { resource_id_of(*source) }
                    .expect("a child was exported with no resource id on it")
            })
            .collect()
    }

    /// The source it created under `resource_id`, for reading properties back
    /// off.
    fn child(&self, resource_id: &str) -> *mut ESource {
        self.created
            .borrow()
            .iter()
            .find(|(id, _)| id == resource_id)
            .map(|(_, source)| source.0)
            .unwrap_or_else(|| panic!("no child was created for {resource_id}"))
    }
}

// SAFETY: every pointer handed out is a valid `ESource` carrying a reference of
// its own, taken with `g_object_ref` just before it is returned.
unsafe impl jmap_backend_collection::fan_out::Collection for Collection {
    fn new_child(&self, resource_id: &str) -> *mut ESource {
        self.calls.borrow_mut().push("new_child");

        if self.refused.iter().any(|id| id == resource_id) {
            return ptr::null_mut();
        }
        if let Some((_, cached)) = self.cached.iter().find(|(id, _)| id == resource_id) {
            return cached.dup();
        }

        let source = Source::new(&format!("jmap-{resource_id}"));
        let handed_out = source.dup();
        self.created
            .borrow_mut()
            .push((resource_id.to_owned(), source));
        handed_out
    }

    fn is_new_child(&self, child: *mut ESource) -> bool {
        // EDS's own rule: a source created by this populate is new, one drawn
        // from the cache of previous sessions is not.
        self.created
            .borrow()
            .iter()
            .any(|(_, source)| source.0 == child)
    }

    fn publish(&self, child: *mut ESource) {
        self.calls.borrow_mut().push("publish");
        self.published.borrow_mut().push(child);
    }

    fn existing_children(&self) -> Vec<*mut ESource> {
        self.calls.borrow_mut().push("existing_children");
        self.existing.iter().map(Source::dup).collect()
    }
}

/// Where the *account* says its server is. Deliberately not the mock's address:
/// the children have to be written from the connection the collection source was
/// read into, and a test whose two answers were the same string could not tell
/// that from a child written out of whatever URL discovery happened to use.
fn connection() -> Connection {
    Connection {
        host: "jmap.example.com".to_owned(),
        port: Some(8443),
        user: Some("vera@example.com".to_owned()),
        auth_method: Some("plain/password".to_owned()),
        secure: true,
    }
}

fn account() -> ServiceAccount {
    ServiceAccount {
        id: Id::new(DEFAULT_ACCOUNT_ID),
        name: "Vera Vibes".to_owned(),
        read_only: false,
    }
}

fn resource(id: &str) -> Resource {
    Resource {
        id: Id::new(id),
        name: format!("Collection {id}"),
        is_default: false,
        color: None,
        writable: None,
    }
}

/// A login serving both collection kinds, with the vectors gated the way
/// [`Fanout::discover`] would have gated them.
fn fanout(parts: Parts, address_books: &[&str], calendars: &[&str]) -> Fanout {
    Fanout {
        parts,
        layout: CollectionLayout {
            mail: None,
            contacts: Some(account()),
            calendars: Some(account()),
        },
        address_books: address_books
            .iter()
            .copied()
            .filter(|_| parts.contacts)
            .map(resource)
            .collect(),
        calendars: calendars
            .iter()
            .copied()
            .filter(|_| parts.calendars)
            .map(resource)
            .collect(),
    }
}

/// A child source in the state a populate leaves it in.
fn written_child(kind: ChildKind, collection: &str) -> Source {
    let resource_id = kind.resource_id(&Id::new(collection));
    let source = Source::new(&format!("jmap-{resource_id}"));
    let child = jmap_collection_sync::Child {
        resource_id,
        kind,
        display_name: format!("Collection {collection}"),
        account_id: Id::new(DEFAULT_ACCOUNT_ID),
        collection_id: Id::new(collection),
        is_default: false,
        color: None,
        read_only: false,
    };
    // SAFETY: a live source of this process's own.
    unsafe {
        jmap_backend_collection::child_source::apply(source.0, &child.settings(&connection()))
    }
    .expect("a child this backend wrote");
    source
}

fn login(origin: &str, parts: Parts) -> Login {
    Login {
        server: Server {
            target: ConnectTarget::Origin(origin.to_owned()),
            connection: connection(),
            rebase_urls: false,
        },
        parts,
        credentials: Credentials::none(),
    }
}

/// Runs `f` against the mock's default account.
fn with_account(server: &MockServer, f: impl FnOnce(&mut AccountState)) {
    let state = server.state();
    let mut state: std::sync::MutexGuard<'_, ServerState> =
        state.lock().expect("the mock server thread is alive");
    let id = Id::new(DEFAULT_ACCOUNT_ID);
    f(state.account_mut(&id).expect("the default account"));
}

/// The `[Authentication]` host and port written onto a child.
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

#[test]
fn every_collection_the_login_holds_becomes_a_child_of_the_collection() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The whole point of M6, end to end: one login, and every address book and
        // calendar the server lists is a child source with the account's server
        // written into it.
        let server = MockServer::builder().start();
        with_account(&server, |account| {
            account.seed_address_book("Personal", true);
            account.seed_address_book("Shared", false);
            account.seed_calendar("Work", true);
        });

        let collection = Collection::default();
        // SAFETY: the collection satisfies the trait's contract.
        let report = unsafe { fan_out(&collection, &login(server.origin(), Parts::ALL)) }
            .expect("the mock answers every listing the fan-out sends");

        assert_eq!(report.children.len(), 3, "{report:?}");
        assert_eq!(collection.created_ids(), report.children);
        assert_eq!(
            collection.published_ids(),
            report.children,
            "a child that is created and not exported is a child Evolution cannot see"
        );
        assert_eq!(
            report,
            Populated {
                children: report.children.clone(),
                ..Populated::default()
            },
            "nothing failed, so nothing is reported as having failed"
        );
    });
}

#[test]
fn every_child_reads_back_under_the_resource_id_it_was_created_under() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The pairing EDS relies on, over ids a real server handed out: the string
        // the fan-out passed to `e_collection_backend_new_child` is the string
        // `dup_resource_id` has to answer for the child that came back. A child
        // whose id does not come back is a child EDS deletes the cache of.
        let server = MockServer::builder().start();
        with_account(&server, |account| {
            account.seed_address_book("Personal", true);
            account.seed_calendar("Work", true);
        });

        let collection = Collection::default();
        // SAFETY: as above.
        let report = unsafe { fan_out(&collection, &login(server.origin(), Parts::ALL)) }
            .expect("the mock answers every listing the fan-out sends");

        assert_eq!(report.children.len(), 2, "{report:?}");
        for resource_id in &report.children {
            let source = collection.child(resource_id);
            assert_eq!(
                // SAFETY: a source this collection still holds a reference to.
                unsafe { resource_id_of(source) }.as_deref(),
                Some(resource_id.as_str()),
            );
        }
    });
}

#[test]
fn each_child_is_written_with_the_server_the_account_names() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Not the URL discovery used: the children are written from the connection
        // the collection `ESource` was read into, which is the same read the origin
        // came out of. A child pointed at anything else is a child that fetches its
        // contacts from a server the account never named.
        let server = MockServer::builder().start();
        with_account(&server, |account| {
            account.seed_address_book("Personal", true);
        });

        let collection = Collection::default();
        // SAFETY: as above.
        let report = unsafe { fan_out(&collection, &login(server.origin(), Parts::ALL)) }
            .expect("the mock answers every listing the fan-out sends");

        let source = collection.child(&report.children[0]);
        assert_eq!(
            host_and_port(source),
            (Some("jmap.example.com".to_owned()), 8443)
        );
        assert_eq!(
            display_name(source).as_deref(),
            Some("Personal"),
            "the sidebar row is named the way the server names the collection"
        );
    });
}

#[test]
fn a_child_created_now_is_exported_and_one_drawn_from_the_cache_is_not() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `e_collection_backend_new_child` draws from a cache of previous sessions,
        // and a child that came out of it was already exported by the `populate`
        // that claimed it. EDS's own collection backend exports only new sources;
        // this holds that line, because the alternative is a second
        // `add_source` for a source the registry already has.
        let cached_id = ChildKind::AddressBook.resource_id(&Id::new("AB1"));
        let collection = Collection {
            cached: vec![(cached_id.clone(), Source::new("jmap-cached"))],
            ..Collection::default()
        };

        // SAFETY: as above.
        let report = unsafe {
            apply_fanout(
                &collection,
                &fanout(Parts::ALL, &["AB1", "AB2"], &[]),
                &connection(),
            )
        };

        assert_eq!(report.children.len(), 2, "both children were written");
        assert_eq!(
            collection.published_ids(),
            [ChildKind::AddressBook.resource_id(&Id::new("AB2"))],
            "only the child this fan-out created is exported"
        );
        assert_eq!(
            // SAFETY: the cached source is alive for the length of this test.
            unsafe { resource_id_of(collection.cached[0].1.0) }.as_deref(),
            Some(cached_id.as_str()),
            "a cached child is written again, so a collection renamed on the \
             server reaches the sidebar"
        );
    });
}

#[test]
fn nothing_is_exported_before_all_of_it_is_written() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The rule `child_source` exists for, followed through: a setting that
        // cannot be written leaves a child missing whichever property makes it work
        // — `[Resource] Identity`, whose absence deletes the cache, or
        // `[Authentication] Host`, whose absence points it at no server. A child
        // that is never exported has neither problem.
        let collection = Collection::default();
        let unwritable = [
            Setting {
                group: EXTENSION_AUTHENTICATION,
                key: "Host",
                value: "jmap.example.com".to_owned(),
            },
            Setting {
                group: EXTENSION_AUTHENTICATION,
                key: "Port",
                value: "not a port".to_owned(),
            },
        ];

        // SAFETY: as above.
        let adopted = unsafe { adopt(&collection, "addressbook:AB1", &unwritable) };

        assert!(
            matches!(adopted, Adopted::Abandoned(_)),
            "{adopted:?} is not an abandoned child"
        );
        assert!(
            collection.published.borrow().is_empty(),
            "a half-written child was exported"
        );
    });
}

#[test]
fn adopt_releases_exactly_the_reference_new_child_handed_over() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `e_collection_backend_new_child` is `(transfer full)`: a reference
        // `adopt` keeps past its own return leaks the source for the life of the
        // account, and one it releases twice frees a source that is still held —
        // by the registry server, for a newly created and exported child, or by
        // this collection's own cache, for one drawn from it.
        //
        // No settings are written here (`&[]`): writing even one creates the EDS
        // extension that holds it, and that extension takes a reference of its
        // own on the source — a real, permanent reference, but EDS's, not one
        // `adopt`'s own transfer-full contract is answerable for. An empty
        // settings list isolates the one reference this test is about.
        let cached_id = ChildKind::AddressBook.resource_id(&Id::new("AB1"));
        let new_id = ChildKind::AddressBook.resource_id(&Id::new("AB2"));
        let collection = Collection {
            cached: vec![(cached_id.clone(), Source::new("jmap-cached"))],
            ..Collection::default()
        };

        // SAFETY: the collection satisfies the trait's contract.
        let cached_adopted = unsafe { adopt(&collection, &cached_id, &[]) };
        assert_eq!(cached_adopted, Adopted::Written { published: false });
        let (_, source) = collection
            .cached
            .iter()
            .find(|(id, _)| *id == cached_id)
            .expect("the cached child is still in the fixture");
        assert_eq!(
            source.ref_count(),
            1,
            "a child drawn from the cache kept the reference `new_child` handed over, \
             or lost its own"
        );

        // SAFETY: as above.
        let new_adopted = unsafe { adopt(&collection, &new_id, &[]) };
        assert_eq!(new_adopted, Adopted::Written { published: true });
        let created = collection.created.borrow();
        let (_, source) = created
            .iter()
            .find(|(id, _)| *id == new_id)
            .expect("AB2 was created");
        assert_eq!(
            source.ref_count(),
            1,
            "a newly created, exported child kept the reference `new_child` handed over, \
             or lost its own"
        );
    });
}

#[test]
fn a_resource_id_eds_refuses_costs_that_child_and_no_other() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `e_collection_backend_new_child` warns and answers NULL when it cannot
        // claim a resource. One child missing is a row missing from the sidebar; a
        // fan-out abandoned at the first NULL is an account missing.
        let refused = ChildKind::AddressBook.resource_id(&Id::new("AB1"));
        let collection = Collection::refusing(&refused);

        // SAFETY: as above.
        let report = unsafe {
            apply_fanout(
                &collection,
                &fanout(Parts::ALL, &["AB1", "AB2"], &["Cal1"]),
                &connection(),
            )
        };

        assert_eq!(report.uncreated, [refused]);
        assert_eq!(
            report.children,
            [
                ChildKind::AddressBook.resource_id(&Id::new("AB2")),
                ChildKind::Calendar.resource_id(&Id::new("Cal1")),
            ]
        );
    });
}

#[test]
fn the_children_the_login_no_longer_warrants_are_removed() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The other half of a populate. These sources have no D-Bus object, so EDS
        // refuses the removal — the one branch this machine can drive — but which
        // children it was *asked* to remove is the decision, and the report is where
        // it is visible.
        let collection = Collection {
            existing: vec![
                written_child(ChildKind::AddressBook, "AB2"),
                written_child(ChildKind::AddressBook, "AB1"),
            ],
            ..Collection::default()
        };

        // SAFETY: as above.
        let report = unsafe {
            apply_fanout(
                &collection,
                &fanout(Parts::ALL, &["AB1"], &[]),
                &connection(),
            )
        };

        let attempted: Vec<&str> = report
            .not_removed
            .iter()
            .map(|failure| failure.resource_id.as_str())
            .collect();
        assert_eq!(
            attempted,
            [ChildKind::AddressBook.resource_id(&Id::new("AB2"))],
            "the child whose collection the server still lists must stay"
        );
    });
}

#[test]
fn the_children_a_collection_has_are_listed_before_any_new_one_is_created() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The order the removal's correctness rests on. A list taken after the new
        // children were added would contain children this same fan-out created, and
        // what keeps them from being removed would be an accident of what
        // `Fanout::is_obsolete` happens to answer rather than of what was asked.
        let collection = Collection::default();

        // SAFETY: as above.
        unsafe {
            apply_fanout(
                &collection,
                &fanout(Parts::ALL, &["AB1"], &[]),
                &connection(),
            )
        };

        let calls = collection.calls.borrow();
        assert_eq!(
            calls.first(),
            Some(&"existing_children"),
            "the children were listed after something else happened: {calls:?}"
        );
    });
}

#[test]
fn a_part_the_user_switched_off_creates_no_child_and_removes_none() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The dormant case, at the layer that acts on it: contacts off means the
        // address books were never listed, so there is nothing to create — and the
        // children of that part are kept, because the cache and the uid are the
        // user's and switching contacts back on has to bring the same source back.
        let collection = Collection {
            existing: vec![written_child(ChildKind::AddressBook, "AB1")],
            ..Collection::default()
        };
        let contacts_off = Parts {
            contacts: false,
            ..Parts::ALL
        };

        // SAFETY: as above.
        let report = unsafe {
            apply_fanout(
                &collection,
                &fanout(contacts_off, &["AB1"], &[]),
                &connection(),
            )
        };

        assert_eq!(report, Populated::default(), "{report:?}");
        assert!(collection.created_ids().is_empty());
    });
}

#[test]
fn a_login_that_cannot_be_reached_touches_no_child() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The fan-out's error is the connection's, and it happens before anything is
        // listed, created or removed: a populate whose server was down must not be
        // the populate that empties the sidebar.
        let collection = Collection {
            existing: vec![written_child(ChildKind::AddressBook, "AB1")],
            ..Collection::default()
        };

        // Port 1 on loopback: nothing listens, and the refusal is immediate.
        // SAFETY: as above.
        let failure = unsafe { fan_out(&collection, &login("http://127.0.0.1:1", Parts::ALL)) };

        assert!(
            failure.is_err(),
            "a dead server answered a session document"
        );
        assert!(
            collection.calls.borrow().is_empty(),
            "the collection was touched"
        );
        assert_eq!(
            // SAFETY: the source is alive for the length of this test.
            unsafe { resource_id_of(collection.existing[0].0) }.as_deref(),
            Some(ChildKind::AddressBook.resource_id(&Id::new("AB1")).as_str()),
            "the child that was there is still there"
        );
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_fan_out_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}
