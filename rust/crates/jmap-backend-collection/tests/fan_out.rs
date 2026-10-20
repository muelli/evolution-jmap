// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests for the fan-out: the changes that one authenticated login makes to the
// children of a collection. The tests use real `ESource`s and a real `jmap-mockd`.
//
// A trait holds the five `ECollectionBackend` methods of the fan-out, so the
// collection here is a test double. All other parts are real:
//
// - `e_source_new_with_uid` makes the sources, as EDS does for a keyfile source.
// - The real backend function `apply` writes the settings.
// - `resource_id_of`, which the `dup_resource_id` vfunc also uses, reads the
//   resource ids.
// - The collections come from a running mock server.
//
// Only the part that needs a session bus is a test double.

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

/// A live `ESource` with the one reference that `e_source_new_with_uid` gives.
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

    /// A new reference. The two `ECollectionBackend` getters of the trait give
    /// one, and the fan-out consumes it.
    fn dup(&self) -> *mut ESource {
        // SAFETY: a live GObject this test holds a reference to.
        unsafe { g_object_ref(self.0.cast()) }.cast()
    }

    /// The GObject reference count. A test asserts it, so that a reference that
    /// `adopt` does not release, or releases twice, causes a failure.
    fn ref_count(&self) -> u32 {
        // SAFETY: a live GObject. `ref_count` is a field of the instance struct.
        unsafe { (*self.0.cast::<GObject>()).ref_count }
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: this is the last reference. The call under test consumed each
        // reference that the test gave out.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// A test double for the `ECollectionBackend` calls of a fan-out.
#[derive(Default)]
struct Collection {
    /// The existing children of this collection, in the order that
    /// `e_collection_backend_list_*_sources` gives.
    existing: Vec<Source>,
    /// Resource ids that `new_child` takes from the cache of previous sessions,
    /// not creates. These children are not new, so `populate` already exported
    /// them.
    cached: Vec<(String, Source)>,
    /// Resource ids for which `new_child` returns NULL.
    refused: Vec<String>,
    /// Each created child, in order.
    created: RefCell<Vec<(String, Source)>>,
    /// Each child given to `e_source_registry_server_add_source`.
    published: RefCell<Vec<*mut ESource>>,
    /// The trait methods in call order, so that the tests can assert the order.
    calls: RefCell<Vec<&'static str>>,
    /// Each `set_remote_deletable` call, by the resource id of its child. The
    /// double records the calls and does not set the `ESource` property. These
    /// fixtures are plain sources, not the `EServerSideSource`s that
    /// `e_server_side_source_set_remote_deletable` needs.
    remote_deletable: RefCell<Vec<(String, bool)>>,
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
                // SAFETY: this collection still holds a reference to each
                // published pointer.
                unsafe { resource_id_of(*source) }
                    .expect("a child was exported with no resource id on it")
            })
            .collect()
    }

    /// The created source for `resource_id`, so that a test can read its
    /// properties.
    fn child(&self, resource_id: &str) -> *mut ESource {
        self.created
            .borrow()
            .iter()
            .find(|(id, _)| id == resource_id)
            .map(|(_, source)| source.0)
            .unwrap_or_else(|| panic!("no child was created for {resource_id}"))
    }

    /// The last `deletable` argument of `set_remote_deletable` for
    /// `resource_id`, or `None` if there was no call for it.
    fn remote_deletable_of(&self, resource_id: &str) -> Option<bool> {
        self.remote_deletable
            .borrow()
            .iter()
            .rev()
            .find(|(id, _)| id == resource_id)
            .map(|(_, deletable)| *deletable)
    }
}

// SAFETY: each returned pointer is a valid `ESource` with its own reference,
// from `g_object_ref` immediately before the return.
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
        // The EDS rule: a source that this populate created is new. A source
        // from the cache of previous sessions is not new.
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

    fn set_remote_deletable(&self, child: *mut ESource, deletable: bool) {
        self.calls.borrow_mut().push("set_remote_deletable");
        // SAFETY: each source here comes from `new_child`, a previous
        // `existing_children` or the cache, and is alive for this call.
        // `unwrap_or_default` is necessary because a test that gives `adopt` no
        // settings writes no identity, and this call still occurs.
        let resource_id = unsafe { resource_id_of(child) }.unwrap_or_default();
        self.remote_deletable
            .borrow_mut()
            .push((resource_id, deletable));
    }
}

/// The server that the account names. This is intentionally not the address of
/// the mock. The children must get the connection from the collection source,
/// and different strings let a test see if discovery supplied the URL instead.
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
        deletable: None,
    }
}

/// A login with both collection kinds. The parts filter the vectors as
/// [`Fanout::discover`] does.
fn fanout(parts: Parts, address_books: &[&str], calendars: &[&str]) -> Fanout {
    Fanout {
        shared: Vec::new(),
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

/// A child source in the state that a populate gives it.
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
        remote_deletable: true,
    };
    // SAFETY: a live source that this process owns.
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

/// Runs `f` on the default account of the mock.
fn with_account(server: &MockServer, f: impl FnOnce(&mut AccountState)) {
    let state = server.state();
    let mut state: std::sync::MutexGuard<'_, ServerState> =
        state.lock().expect("the mock server thread is alive");
    let id = Id::new(DEFAULT_ACCOUNT_ID);
    f(state.account_mut(&id).expect("the default account"));
}

/// The `[Authentication]` host and port of a child.
fn host_and_port(source: *mut ESource) -> (Option<String>, u16) {
    // SAFETY: no arguments, and the type system initialises itself.
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
    // SAFETY: a live source. The source owns the name that it returns.
    unsafe { jmap_backend_core::marshal::read_string(e_source_get_display_name(source)) }
}

#[test]
fn every_collection_the_login_holds_becomes_a_child_of_the_collection() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The full fan-out: after one login, each address book and calendar from
        // the server is a child source with the server of the account.
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
        // EDS needs this pairing. For each child, `dup_resource_id` must return
        // the string that the fan-out gave to `e_collection_backend_new_child`.
        // If not, EDS deletes the cache of the child.
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
        // The children get the connection from the collection `ESource`, not
        // from the discovery URL. The origin comes from the same read. Otherwise
        // a child could get contacts from a server that the account does not name.
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

/// A fan-out of only the address books of one account. A test can then set the
/// `deletable` field of each `Resource`. `fanout()` always sets `deletable` to
/// `None`.
fn fanout_of(resources: Vec<Resource>) -> Fanout {
    Fanout {
        shared: Vec::new(),
        parts: Parts::ALL,
        layout: CollectionLayout {
            mail: None,
            contacts: Some(account()),
            calendars: None,
        },
        address_books: resources,
        calendars: Vec::new(),
    }
}

#[test]
fn a_childs_may_delete_reaches_set_remote_deletable_and_absence_stays_deletable() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `myRights.mayDelete` goes to the EDS `remote-deletable` through
        // `Resource::deletable` -> `Child::remote_deletable` ->
        // `Collection::set_remote_deletable`. An absent `myRights` is the usual
        // case, and it must not mean "not deletable".
        let mut locked = resource("AB1");
        locked.deletable = Some(false);
        let collection = Collection::default();

        // SAFETY: as above.
        unsafe {
            apply_fanout(
                &collection,
                &fanout_of(vec![locked, resource("AB2")]),
                &connection(),
            )
        };

        assert_eq!(
            collection.remote_deletable_of(&ChildKind::AddressBook.resource_id(&Id::new("AB1"))),
            Some(false),
            "myRights said not deletable"
        );
        assert_eq!(
            collection.remote_deletable_of(&ChildKind::AddressBook.resource_id(&Id::new("AB2"))),
            Some(true),
            "absent myRights stays deletable"
        );
    });
}

#[test]
fn a_rediscovery_with_changed_rights_updates_remote_deletable_on_the_existing_child() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `offer_deletion` in `child_added` runs only once, when a child is
        // first exported. Thus `adopt`, which runs on each fan-out, must apply a
        // change of rights to an existing child.
        let cached_id = ChildKind::AddressBook.resource_id(&Id::new("AB1"));
        let collection = Collection {
            cached: vec![(cached_id.clone(), Source::new("jmap-cached"))],
            ..Collection::default()
        };

        let mut deletable = resource("AB1");
        deletable.deletable = Some(true);
        // SAFETY: as above.
        unsafe { apply_fanout(&collection, &fanout_of(vec![deletable]), &connection()) };
        assert_eq!(collection.remote_deletable_of(&cached_id), Some(true));

        let mut now_locked = resource("AB1");
        now_locked.deletable = Some(false);
        // SAFETY: as above.
        unsafe { apply_fanout(&collection, &fanout_of(vec![now_locked]), &connection()) };
        assert_eq!(
            collection.remote_deletable_of(&cached_id),
            Some(false),
            "a rights change on an existing child must reach it on rediscovery, \
             not only on the child's first mint"
        );
    });
}

#[test]
fn a_child_created_now_is_exported_and_one_drawn_from_the_cache_is_not() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `e_collection_backend_new_child` can return a child from the cache of
        // previous sessions. The `populate` that claimed that child exported it.
        // Like the EDS collection backend, this backend exports only new sources.
        // Otherwise `add_source` would occur twice for one source.
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
            // SAFETY: the cached source is alive for the duration of this test.
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
        // The rule of `child_source`: if a setting cannot be written, the child
        // can be without a necessary property. Without `[Resource] Identity`, EDS
        // deletes the cache. Without `[Authentication] Host`, the child has no
        // server. A child that is not exported has neither problem.
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
        let adopted = unsafe { adopt(&collection, "addressbook:AB1", &unwritable, true) };

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
        // `e_collection_backend_new_child` is `(transfer full)`. If `adopt`
        // keeps the reference after it returns, the source leaks. If it releases
        // the reference twice, it frees a source that a different owner holds.
        // That owner is the registry server for a new exported child, or the
        // cache for a cached child.
        //
        // This test writes no settings (`&[]`). One setting creates an EDS
        // extension, and that extension holds its own reference on the source.
        // That reference belongs to EDS, not to `adopt`.
        let cached_id = ChildKind::AddressBook.resource_id(&Id::new("AB1"));
        let new_id = ChildKind::AddressBook.resource_id(&Id::new("AB2"));
        let collection = Collection {
            cached: vec![(cached_id.clone(), Source::new("jmap-cached"))],
            ..Collection::default()
        };

        // SAFETY: the collection satisfies the trait's contract.
        let cached_adopted = unsafe { adopt(&collection, &cached_id, &[], true) };
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
        let new_adopted = unsafe { adopt(&collection, &new_id, &[], true) };
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
        // `e_collection_backend_new_child` warns and returns NULL when it cannot
        // claim a resource. Then only that child is missing from the sidebar. The
        // fan-out must not stop at the first NULL.
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
        // The removal part of a populate. These sources have no D-Bus object, so
        // EDS refuses the removal. The test checks which children the fan-out
        // tried to remove, as the report shows.
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
        // The removal is correct only with this order. A list made after the
        // creation would contain the new children of this fan-out. Only the
        // answer of `Fanout::is_obsolete` would then prevent their removal.
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
        // The dormant case. With contacts disabled, the fan-out does not list
        // address books, so it creates none. It keeps the children of that part,
        // so that the same source with its cache and uid comes back later.
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
        // The connection error occurs before the fan-out lists, creates or removes
        // children. Thus a server that is down does not empty the sidebar.
        let collection = Collection {
            existing: vec![written_child(ChildKind::AddressBook, "AB1")],
            ..Collection::default()
        };

        // No service listens on loopback port 1, so the refusal is immediate.
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
            // SAFETY: the source is alive for the duration of this test.
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
