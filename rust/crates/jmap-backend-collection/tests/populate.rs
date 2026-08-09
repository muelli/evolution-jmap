// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The offline half of a populate: the cached children of previous sessions, back
// in the sidebar before anything is asked of the server.
//
// The `ECollectionBackend` calls a populate makes are a trait, as in
// `tests/fan_out.rs`, so the collection here is a stand-in — but the sources are
// real `ESource`s built by `e_source_new_with_uid`, and their resource ids are
// read back by the same `resource_id_of` the `dup_resource_id` vfunc answers
// with. The freeze counter in the stand-in is EDS's own arithmetic:
// `e_collection_backend_freeze_populate` always increments and answers whether
// *this* caller is the one that got the freeze, which is why a populate that
// loses the race still owes a thaw.

use std::cell::{Cell, RefCell};
use std::ffi::CString;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_ADDRESS_BOOK, E_SOURCE_EXTENSION_CALENDAR, E_SOURCE_EXTENSION_RESOURCE,
    ESource, ESourceResource, e_source_address_book_get_type, e_source_calendar_get_type,
    e_source_get_extension, e_source_new_with_uid, e_source_resource_set_identity,
};
use gobject_sys::{GObject, g_object_ref, g_object_unref};
use jmap_backend_collection::populate::{Asked, Populating, Restored, populate};
use jmap_backend_collection::resource_id::resource_id_of;
use jmap_collection_sync::{ChildKind, Parts};

/// A live `ESource` this test holds one reference to, standing in for one of the
/// `.source` files in a collection's cache directory.
struct Source(*mut ESource);

impl Source {
    /// A cached child of the given kind, carrying the identity that makes its
    /// resource id readable — which is the state EDS's own
    /// `collection_backend_load_resources` requires before it will cache a
    /// source at all.
    fn cached(kind: ChildKind, identity: &str) -> Self {
        let extension = match kind {
            ChildKind::AddressBook => E_SOURCE_EXTENSION_ADDRESS_BOOK,
            ChildKind::Calendar => E_SOURCE_EXTENSION_CALENDAR,
        };
        let source = Self::bare(&format!("jmap-cached-{identity}"));
        let extension = CString::new(extension.to_bytes()).expect("no NUL in an extension name");
        let identity = CString::new(identity).expect("no NUL in a test identity");
        // SAFETY: a live source and NUL-terminated names; the extensions are
        // created on demand and owned by the source, and the setter copies.
        unsafe {
            assert!(!e_source_get_extension(source.0, extension.as_ptr()).is_null());
            let resource: *mut ESourceResource =
                e_source_get_extension(source.0, E_SOURCE_EXTENSION_RESOURCE.as_ptr()).cast();
            e_source_resource_set_identity(resource, identity.as_ptr());
        }
        source
    }

    /// A source with no extension of ours on it at all, which is what a child of
    /// some other backend — or one a user hand-edited — looks like.
    fn bare(uid: &str) -> Self {
        // SAFETY: no arguments; `e_source_get_extension` cannot find an
        // extension class whose type nothing has referenced yet.
        unsafe {
            e_source_address_book_get_type();
            e_source_calendar_get_type();
        }
        let uid = CString::new(uid).expect("no NUL in a test uid");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a GError
        // out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    /// A reference of its own, which is what `claim_all_resources` hands over
    /// and what the populate consumes.
    fn dup(&self) -> *mut ESource {
        // SAFETY: a live GObject this test holds a reference to.
        unsafe { g_object_ref(self.0.cast()) }.cast()
    }

    /// The GObject reference count, which is how a claim that was not given back
    /// becomes an assertion rather than a leak nobody notices.
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

/// The `ECollectionBackend` half of a populate, without one.
#[derive(Default)]
struct Collection {
    /// EDS's freeze counter, with EDS's arithmetic on it.
    freeze_count: Cell<i32>,
    /// The cache of previous sessions, as `claim_all_resources` hands it over.
    cached: Vec<Source>,
    /// Whether the cache has already been claimed: "previously used sources can
    /// only be claimed once, so subsequent calls to this function for @backend
    /// will return NULL".
    claimed: Cell<bool>,
    /// Which trait method was called when, so an ordering decision is a test
    /// rather than a comment.
    calls: RefCell<Vec<&'static str>>,
    /// Every source handed to `e_source_registry_server_add_source`.
    published: RefCell<Vec<*mut ESource>>,
    /// Makes `publish` panic, for the one thing a populate must do even then.
    publish_panics: bool,
}

impl Collection {
    fn with(cached: Vec<Source>) -> Self {
        Self {
            cached,
            ..Self::default()
        }
    }

    /// The resource ids of the sources it exported, read back off them.
    fn published_ids(&self) -> Vec<String> {
        self.published
            .borrow()
            .iter()
            .map(|source| {
                // SAFETY: every published pointer is one this collection still
                // holds a reference to.
                unsafe { resource_id_of(*source) }
                    .expect("a source was exported with no resource id on it")
            })
            .collect()
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.borrow().clone()
    }
}

// SAFETY: every pointer handed out is a valid `ESource` carrying a reference of
// its own, taken with `g_object_ref` just before it is returned — which is what
// `e_collection_backend_claim_all_resources` does too.
unsafe impl Populating for Collection {
    fn freeze(&self) -> bool {
        self.calls.borrow_mut().push("freeze");
        // `return !g_atomic_int_add (&count, 1)`: the increment happens whatever
        // the answer, so a caller that is told FALSE still owes a thaw.
        let before = self.freeze_count.get();
        self.freeze_count.set(before + 1);
        before == 0
    }

    fn thaw(&self) {
        self.calls.borrow_mut().push("thaw");
        self.freeze_count.set(self.freeze_count.get() - 1);
        assert!(
            self.freeze_count.get() >= 0,
            "the freeze count went negative, which EDS answers with a critical"
        );
    }

    fn chain_up(&self) {
        self.calls.borrow_mut().push("chain_up");
    }

    fn claim_all_resources(&self) -> Vec<*mut ESource> {
        self.calls.borrow_mut().push("claim_all_resources");
        if self.claimed.replace(true) {
            return Vec::new();
        }
        self.cached.iter().map(Source::dup).collect()
    }

    fn publish(&self, child: *mut ESource) {
        self.calls.borrow_mut().push("publish");
        if self.publish_panics {
            panic!("a publish that fails the way a broken registry server would");
        }
        self.published.borrow_mut().push(child);
    }

    fn request_credentials(&self) {
        self.calls.borrow_mut().push("request_credentials");
    }

    fn authenticate_anonymously(&self) {
        self.calls.borrow_mut().push("authenticate_anonymously");
    }
}

/// The populate under test, with the account's two answers spelled out.
fn run(collection: &Collection, parts: Parts, user: Option<&str>) -> Option<Restored> {
    // SAFETY: the stand-in satisfies `Populating`'s contract — see the impl.
    unsafe { populate(collection, parts, user) }
}

#[test]
fn the_cached_children_of_previous_sessions_are_exported_again() {
    // The whole point of the offline half. EDS loads a collection's cached
    // `.source` files into an unclaimed table and exports none of them; until a
    // populate claims them and passes each to
    // `e_source_registry_server_add_source`, the account's address books and
    // calendars are files on disk that Evolution cannot see. A populate that
    // skipped this would leave the sidebar empty until the first successful
    // login, which is exactly the case offline support exists for.
    let collection = Collection::with(vec![
        Source::cached(ChildKind::AddressBook, "B1"),
        Source::cached(ChildKind::Calendar, "C1"),
    ]);

    let report = run(&collection, Parts::ALL, Some("vera@example.com"))
        .expect("nothing else holds the freeze");

    assert_eq!(
        collection.published_ids(),
        ["addressbook:B1", "calendar:C1"]
    );
    assert_eq!(report.children, ["addressbook:B1", "calendar:C1"]);
    assert_eq!(report.unidentified, 0);
}

#[test]
fn the_cache_is_claimed_after_the_chain_up_and_before_any_password_is_asked_for() {
    // Three orderings in one, and each is EDS's:
    //
    // - the freeze comes first, because it is what makes two populates of one
    //   account not run over each other;
    // - the chain-up comes before the work, which is what chaining up means —
    //   `ECollectionBackendClass::populate` is a placeholder in 3.52, so a
    //   populate that never chained up would look identical today and break on
    //   the release that fills it in;
    // - the credentials are asked for last, because they are what brings the
    //   *server's* answer, and the cached children have to be in the sidebar
    //   before a login that may never succeed.
    let collection = Collection::with(vec![Source::cached(ChildKind::AddressBook, "B1")]);

    run(&collection, Parts::ALL, Some("vera@example.com")).expect("nothing holds the freeze");

    assert_eq!(
        collection.calls(),
        [
            "freeze",
            "chain_up",
            "claim_all_resources",
            "publish",
            "request_credentials",
            "thaw",
        ]
    );
}

#[test]
fn every_reference_the_claim_handed_over_is_given_back() {
    // `e_collection_backend_claim_all_resources` is `(transfer full)`, and it
    // hands over one reference per source *and* the list. A populate that kept
    // them would pin every cached child of every account for the life of the
    // process — and one that unreferenced an exported source twice would free a
    // source the registry server is still holding.
    let cached = vec![
        Source::cached(ChildKind::AddressBook, "B1"),
        Source::cached(ChildKind::Calendar, "C1"),
    ];
    let before: Vec<u32> = cached.iter().map(Source::ref_count).collect();
    let collection = Collection::with(cached);

    run(&collection, Parts::ALL, None).expect("nothing holds the freeze");

    let after: Vec<u32> = collection.cached.iter().map(Source::ref_count).collect();
    assert_eq!(
        after, before,
        "the populate kept or dropped too many references"
    );
}

#[test]
fn a_populate_that_lost_the_freeze_does_nothing_but_give_it_back() {
    // EDS's own guard, and the reason it is not `if (frozen) return`:
    // `e_collection_backend_freeze_populate` increments the counter whatever it
    // answers, so the loser of the race owes a thaw. Getting that wrong either
    // way is invisible until it is permanent — a missing thaw freezes the
    // account's populate for the life of the process, and an extra one lets two
    // populates create the same children twice.
    let collection = Collection::with(vec![Source::cached(ChildKind::AddressBook, "B1")]);
    // Another populate, already running.
    assert!(collection.freeze());

    assert_eq!(run(&collection, Parts::ALL, Some("vera@example.com")), None);

    assert_eq!(collection.calls(), ["freeze", "freeze", "thaw"]);
    assert_eq!(
        collection.freeze_count.get(),
        1,
        "the populate that is still running lost its freeze"
    );
    assert!(collection.published.borrow().is_empty());
}

#[test]
fn the_freeze_is_given_back_even_when_the_work_panics() {
    // A populate is called from an idle callback in `evolution-source-registry`,
    // and the panic guard in front of the vfunc turns a Rust panic into a logged
    // critical rather than an unwind into C. What the guard cannot do is undo the
    // freeze: a panic between the freeze and the thaw would leave this account's
    // populate frozen for the life of the process, so the account would never
    // populate again — and never say why.
    let collection = Collection {
        publish_panics: true,
        ..Collection::with(vec![Source::cached(ChildKind::AddressBook, "B1")])
    };

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        run(&collection, Parts::ALL, Some("vera@example.com"))
    }));

    assert!(panicked.is_err(), "this test needs the publish to panic");
    assert_eq!(
        collection.freeze_count.get(),
        0,
        "the freeze outlived the populate that took it"
    );
    assert_eq!(collection.calls().last(), Some(&"thaw"));
}

#[test]
fn an_account_with_nothing_switched_on_still_gets_its_children_back() {
    // The children of a switched-off part are dormant, not gone: EDS binds each
    // child's `enabled` to the account's part flag, so a child of a part the
    // user unticked is exported and shown switched off. A populate that withheld
    // them instead would make them vanish from the sidebar — and, since nothing
    // would then claim their resource ids, the next populate that found the part
    // switched on again would create fresh sources with fresh uids beside the
    // cached files. That is the same destruction `Fanout::is_obsolete` refuses
    // to do, reached from the other side.
    let collection = Collection::with(vec![Source::cached(ChildKind::AddressBook, "B1")]);

    let report =
        run(&collection, Parts::NONE, Some("vera@example.com")).expect("nothing holds the freeze");

    assert_eq!(collection.published_ids(), ["addressbook:B1"]);
    assert_eq!(report.asked, Asked::Nothing);
    assert!(
        !collection.calls().contains(&"request_credentials"),
        "an account with nothing switched on was asked for a password"
    );
}

#[test]
fn an_account_with_only_mail_switched_on_is_not_asked_for_a_password() {
    // This backend creates no mail children yet (see
    // `jmap_collection_sync::children`), so a mail-only account has nothing for
    // a login to discover. Asking EDS for credentials would resolve a password
    // — or prompt for one — to produce nothing the user can see, which is the
    // one thing a populate should never spend a prompt on. EDS's own WebDAV
    // collection backend gates on the same pair of parts, for the same reason:
    // they are the parts it makes children for.
    let collection = Collection::with(vec![]);

    let report = run(
        &collection,
        Parts {
            mail: true,
            contacts: false,
            calendars: false,
        },
        Some("vera@example.com"),
    )
    .expect("nothing holds the freeze");

    assert_eq!(report.asked, Asked::Nothing);
    assert_eq!(
        collection.calls(),
        ["freeze", "chain_up", "claim_all_resources", "thaw"]
    );
}

#[test]
fn one_switched_on_part_is_enough_to_ask_for_a_password() {
    // The other side of the gate above: contacts alone, and calendars alone,
    // each warrant a login. A gate that needed both would leave an account with
    // one part ticked permanently un-authenticated.
    for parts in [
        Parts {
            mail: false,
            contacts: true,
            calendars: false,
        },
        Parts {
            mail: false,
            contacts: false,
            calendars: true,
        },
    ] {
        let collection = Collection::with(vec![]);

        let report =
            run(&collection, parts, Some("vera@example.com")).expect("nothing holds the freeze");

        assert_eq!(report.asked, Asked::Credentials, "{parts:?}");
        assert!(
            collection.calls().contains(&"request_credentials"),
            "{parts:?}"
        );
    }
}

#[test]
fn an_account_that_names_no_user_is_authenticated_without_a_password() {
    // `e_backend_schedule_credentials_required` is how a backend asks for a
    // password; `e_backend_schedule_authenticate` is how it asks to be
    // authenticated *now*, with whatever it already has. An anonymous JMAP
    // account — `credentials()` reads a source that names no user as anonymous
    // on purpose — has no password to resolve, so asking for one would put a
    // prompt in front of someone whose account needs none, and whatever they
    // typed would be dropped on the floor by `credentials()` anyway.
    let collection = Collection::with(vec![]);

    let report = run(&collection, Parts::ALL, None).expect("nothing holds the freeze");

    assert_eq!(report.asked, Asked::Anonymously);
    assert_eq!(
        collection.calls(),
        [
            "freeze",
            "chain_up",
            "claim_all_resources",
            "authenticate_anonymously",
            "thaw",
        ]
    );
}

#[test]
fn a_cached_source_this_backend_cannot_name_is_not_exported() {
    // Unreachable through EDS, and defined anyway: EDS only caches a source
    // whose resource id `dup_resource_id` — this crate's — answered for, so a
    // claimed source that reads back as `None` is a source that changed
    // underneath. Exporting it would put a child in the sidebar that no resource
    // id can ever be paired with again: `e_collection_backend_new_child` finds
    // an existing child by asking `dup_resource_id` about each one, so a child
    // that answers `None` is one every later populate re-creates instead of
    // reusing. It stays unexported, and the count is the only trace there is —
    // `populate` returns `void` and has nobody to report to.
    let collection = Collection::with(vec![
        Source::cached(ChildKind::AddressBook, "B1"),
        Source::bare("jmap-cached-nameless"),
    ]);

    let report = run(&collection, Parts::ALL, None).expect("nothing holds the freeze");

    assert_eq!(collection.published_ids(), ["addressbook:B1"]);
    assert_eq!(report.children, ["addressbook:B1"]);
    assert_eq!(report.unidentified, 1);
}
