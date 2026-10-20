// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests for the offline part of a populate. The cached children of previous
// sessions go back into the sidebar before the backend contacts the server.
//
// A trait holds the `ECollectionBackend` calls of a populate, as in
// `tests/fan_out.rs`, so the collection here is a test double. The sources are
// real `ESource`s from `e_source_new_with_uid`. The `dup_resource_id` vfunc and
// these tests both read resource ids with `resource_id_of`.
//
// The freeze counter of the test double uses the EDS arithmetic.
// `e_collection_backend_freeze_populate` always increments the counter. It
// returns TRUE only to the caller that got the freeze. Thus a populate that does
// not get the freeze must still thaw.

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

mod common;
use common::{with_timeout, with_timeout_duration};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A live `ESource` with one reference held by the test. It represents a
/// `.source` file in the cache directory of a collection.
struct Source(*mut ESource);

impl Source {
    /// A cached child of the given kind, with an identity that gives a readable
    /// resource id. `collection_backend_load_resources` in EDS caches only a
    /// source in this state.
    fn cached(kind: ChildKind, identity: &str) -> Self {
        let extension = match kind {
            ChildKind::AddressBook => E_SOURCE_EXTENSION_ADDRESS_BOOK,
            ChildKind::Calendar => E_SOURCE_EXTENSION_CALENDAR,
        };
        let source = Self::bare(&format!("jmap-cached-{identity}"));
        let extension = CString::new(extension.to_bytes()).expect("no NUL in an extension name");
        let identity = CString::new(identity).expect("no NUL in a test identity");
        // SAFETY: a live source and NUL-terminated names. The source creates the
        // extensions when necessary and owns them. The setter copies its argument.
        unsafe {
            assert!(!e_source_get_extension(source.0, extension.as_ptr()).is_null());
            let resource: *mut ESourceResource =
                e_source_get_extension(source.0, E_SOURCE_EXTENSION_RESOURCE.as_ptr()).cast();
            e_source_resource_set_identity(resource, identity.as_ptr());
        }
        source
    }

    /// A source without the extensions of this backend, like a child of a
    /// different backend or a source that a user edited by hand.
    fn bare(uid: &str) -> Self {
        // SAFETY: no arguments. These calls register the types, because
        // `e_source_get_extension` cannot find an unregistered extension class.
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

    /// A new reference, as `claim_all_resources` gives and the populate consumes.
    fn dup(&self) -> *mut ESource {
        // SAFETY: a live GObject this test holds a reference to.
        unsafe { g_object_ref(self.0.cast()) }.cast()
    }

    /// The GObject reference count. A test asserts it, so that a reference that
    /// the populate does not release causes a failure, not a silent leak.
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

/// A test double for the `ECollectionBackend` calls of a populate.
#[derive(Default)]
struct Collection {
    /// The EDS freeze counter, with the EDS arithmetic.
    freeze_count: Cell<i32>,
    /// The cache of previous sessions, as `claim_all_resources` gives it.
    cached: Vec<Source>,
    /// Whether a call already claimed the cache. The EDS documentation says:
    /// "previously used sources can only be claimed once, so subsequent calls to
    /// this function for @backend will return NULL".
    claimed: Cell<bool>,
    /// The trait methods in call order, so that the tests can assert the order.
    calls: RefCell<Vec<&'static str>>,
    /// Every source handed to `e_source_registry_server_add_source`.
    published: RefCell<Vec<*mut ESource>>,
    /// Makes `publish` panic, to test that the populate still thaws.
    publish_panics: bool,
    /// Each argument of `offer_creation`, in order. This flag tells whether
    /// Evolution can create a collection in this account.
    offered: RefCell<Vec<bool>>,
}

impl Collection {
    fn with(cached: Vec<Source>) -> Self {
        Self {
            cached,
            ..Self::default()
        }
    }

    /// The resource ids of the exported sources, read from the sources.
    fn published_ids(&self) -> Vec<String> {
        self.published
            .borrow()
            .iter()
            .map(|source| {
                // SAFETY: this collection still holds a reference to each
                // published pointer.
                unsafe { resource_id_of(*source) }
                    .expect("a source was exported with no resource id on it")
            })
            .collect()
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.borrow().clone()
    }
}

// SAFETY: each returned pointer is a valid `ESource` with its own reference,
// from `g_object_ref` immediately before the return. This is also what
// `e_collection_backend_claim_all_resources` does.
unsafe impl Populating for Collection {
    fn freeze(&self) -> bool {
        self.calls.borrow_mut().push("freeze");
        // `return !g_atomic_int_add (&count, 1)`: the increment occurs for each
        // answer, so a caller that gets FALSE must still thaw.
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

    fn offer_creation(&self, offer: bool) {
        self.calls.borrow_mut().push("offer_creation");
        self.offered.borrow_mut().push(offer);
    }

    fn allow_rename(&self) {
        self.calls.borrow_mut().push("allow_rename");
    }
}

/// Runs the populate under test with the given account settings.
fn run(
    collection: &Collection,
    parts: Parts,
    user: Option<&str>,
    uses_oauth2: bool,
) -> Option<Restored> {
    // SAFETY: the test double satisfies the contract of `Populating` (see the
    // impl).
    unsafe { populate(collection, parts, user, uses_oauth2) }
}

#[test]
fn an_oauth2_account_is_authenticated_silently_rather_than_prompted() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `e_backend_schedule_credentials_required` sends an OAuth 2.0 source
        // directly to the consent window. On that path, the EDS OAuth2
        // credentials provider cannot find or store a token
        // (e-source-credentials-provider-impl-oauth2.c answers can_store FALSE
        // and can_prompt TRUE). `authenticate_sync` does not need resolved
        // credentials, because `login_of` gets the token silently. Thus an
        // OAuth 2.0 account uses `e_backend_schedule_authenticate`, as an
        // anonymous account does. The credentials-required path would open the
        // consent window at each populate.
        let collection = Collection::with(vec![]);

        let report = run(&collection, Parts::ALL, Some("vera@example.com"), true)
            .expect("nothing holds the freeze");

        assert_eq!(report.asked, Asked::Silently);
        assert!(
            collection.calls().contains(&"authenticate_anonymously"),
            "the OAuth2 account must be driven to authenticate_sync, not to a prompt"
        );
        assert!(
            !collection.calls().contains(&"request_credentials"),
            "scheduling credentials-required opens the consent window"
        );
    });
}

#[test]
fn the_cached_children_of_previous_sessions_are_exported_again() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // This is the purpose of the offline part. EDS loads the cached
        // `.source` files of a collection into an unclaimed table and exports
        // none of them. Evolution cannot see them until a populate claims them
        // and gives each to `e_source_registry_server_add_source`. Without this,
        // the sidebar stays empty until the first successful login.
        let collection = Collection::with(vec![
            Source::cached(ChildKind::AddressBook, "B1"),
            Source::cached(ChildKind::Calendar, "C1"),
        ]);

        let report = run(&collection, Parts::ALL, Some("vera@example.com"), false)
            .expect("nothing else holds the freeze");

        assert_eq!(
            collection.published_ids(),
            ["addressbook:B1", "calendar:C1"]
        );
        assert_eq!(report.children, ["addressbook:B1", "calendar:C1"]);
        assert_eq!(report.unidentified, 0);
    });
}

#[test]
fn the_cache_is_claimed_after_the_chain_up_and_before_any_password_is_asked_for() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // This test checks three EDS order rules:
        //
        // - The freeze is first. It prevents two populates of one account from
        //   running at the same time.
        // - The chain-up is before the work. `ECollectionBackendClass::populate`
        //   is a placeholder in 3.52, but a later release can give it a function.
        // - The credentials request is last. The cached children must be in the
        //   sidebar before a login that possibly fails.
        let collection = Collection::with(vec![Source::cached(ChildKind::AddressBook, "B1")]);

        run(&collection, Parts::ALL, Some("vera@example.com"), false)
            .expect("nothing holds the freeze");

        assert_eq!(
            collection.calls(),
            [
                "freeze",
                "chain_up",
                "claim_all_resources",
                "publish",
                "offer_creation",
                "allow_rename",
                "request_credentials",
                "thaw",
            ]
        );
    });
}

#[test]
fn every_reference_the_claim_handed_over_is_given_back() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `e_collection_backend_claim_all_resources` is `(transfer full)`: it
        // gives one reference for each source and the list. If the populate kept
        // them, each cached child would stay in memory until the process stops.
        // If it released an exported source twice, it would free a source that
        // the registry server still holds.
        let cached = vec![
            Source::cached(ChildKind::AddressBook, "B1"),
            Source::cached(ChildKind::Calendar, "C1"),
        ];
        let before: Vec<u32> = cached.iter().map(Source::ref_count).collect();
        let collection = Collection::with(cached);

        run(&collection, Parts::ALL, None, false).expect("nothing holds the freeze");

        let after: Vec<u32> = collection.cached.iter().map(Source::ref_count).collect();
        assert_eq!(
            after, before,
            "the populate kept or dropped too many references"
        );
    });
}

#[test]
fn a_populate_that_lost_the_freeze_does_nothing_but_give_it_back() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // This is the EDS guard, and it is not `if (frozen) return`.
        // `e_collection_backend_freeze_populate` increments the counter for each
        // answer, so a populate that does not get the freeze must still thaw. A
        // missing thaw freezes the populate of the account until the process
        // stops. An extra thaw lets two populates create the same children twice.
        let collection = Collection::with(vec![Source::cached(ChildKind::AddressBook, "B1")]);
        // A different populate that is already running.
        assert!(collection.freeze());

        assert_eq!(
            run(&collection, Parts::ALL, Some("vera@example.com"), false),
            None
        );

        assert_eq!(collection.calls(), ["freeze", "freeze", "thaw"]);
        assert_eq!(
            collection.freeze_count.get(),
            1,
            "the populate that is still running lost its freeze"
        );
        assert!(collection.published.borrow().is_empty());
    });
}

#[test]
fn the_freeze_is_given_back_even_when_the_work_panics() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // An idle callback in `evolution-source-registry` calls the populate. The
        // panic guard of the vfunc changes a Rust panic into a logged critical,
        // not an unwind into C. But the guard cannot undo the freeze. A panic
        // between the freeze and the thaw would stop all later populates of this
        // account, with no message.
        let collection = Collection {
            publish_panics: true,
            ..Collection::with(vec![Source::cached(ChildKind::AddressBook, "B1")])
        };

        let panicked = catch_unwind(AssertUnwindSafe(|| {
            run(&collection, Parts::ALL, Some("vera@example.com"), false)
        }));

        assert!(panicked.is_err(), "this test needs the publish to panic");
        assert_eq!(
            collection.freeze_count.get(),
            0,
            "the freeze outlived the populate that took it"
        );
        assert_eq!(collection.calls().last(), Some(&"thaw"));
    });
}

#[test]
fn an_account_with_nothing_switched_on_still_gets_its_children_back() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The children of a disabled part are dormant, not removed. EDS binds the
        // `enabled` property of each child to the part flag of the account, so
        // the sidebar shows these children as disabled. If the populate did not
        // export them, they would disappear from the sidebar. Also, no source
        // would claim their resource ids. When the user enables the part again,
        // the next populate would create new sources with new uids, next to the
        // cached files. `Fanout::is_obsolete` prevents the same damage.
        let collection = Collection::with(vec![Source::cached(ChildKind::AddressBook, "B1")]);

        let report = run(&collection, Parts::NONE, Some("vera@example.com"), false)
            .expect("nothing holds the freeze");

        assert_eq!(collection.published_ids(), ["addressbook:B1"]);
        assert_eq!(report.asked, Asked::Nothing);
        assert!(
            !collection.calls().contains(&"request_credentials"),
            "an account with nothing switched on was asked for a password"
        );
    });
}

#[test]
fn an_account_with_only_mail_switched_on_is_not_asked_for_a_password() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // This backend does not create mail children (see
        // `jmap_collection_sync::children`). Thus a login for a mail-only account
        // has nothing to discover, and a password prompt would give no result.
        // The EDS WebDAV collection backend uses the same two parts as its
        // condition, because it makes children only for them.
        let collection = Collection::with(vec![]);

        let report = run(
            &collection,
            Parts {
                mail: true,
                contacts: false,
                calendars: false,
            },
            Some("vera@example.com"),
            false,
        )
        .expect("nothing holds the freeze");

        assert_eq!(report.asked, Asked::Nothing);
        assert_eq!(
            collection.calls(),
            [
                "freeze",
                "chain_up",
                "claim_all_resources",
                "offer_creation",
                "allow_rename",
                "thaw"
            ]
        );
    });
}

#[test]
fn an_account_that_makes_children_is_offered_as_a_place_to_create_them() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Without this flag, `create_resource_sync` never runs.
        // `server_side_source_remote_create_sync` refuses a collection source
        // without `remote-creatable`. Thus an account with address books or
        // calendars must get this flag.
        for parts in [
            Parts::ALL,
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

            let report = run(&collection, parts, Some("vera@example.com"), false)
                .expect("nothing holds the freeze");

            assert!(report.creatable, "{parts:?} makes children of this backend");
            assert_eq!(collection.offered.borrow().as_slice(), [true]);
        }
    });
}

#[test]
fn an_account_that_makes_no_children_is_not_offered_as_a_place_to_create_them() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The password request uses the same condition. An account with contacts
        // and calendars disabled has no children of this backend. The next
        // populate would make a new collection in it dormant. The populate writes
        // FALSE explicitly, because the user can change the parts at any time.
        // The flag must follow that change without removal of the account.
        let collection = Collection::with(vec![]);

        let report = run(
            &collection,
            Parts {
                mail: true,
                contacts: false,
                calendars: false,
            },
            Some("vera@example.com"),
            false,
        )
        .expect("nothing holds the freeze");

        assert!(!report.creatable);
        assert_eq!(collection.offered.borrow().as_slice(), [false]);
    });
}

#[test]
fn one_switched_on_part_is_enough_to_ask_for_a_password() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The opposite case: contacts alone, or calendars alone, is sufficient for
        // a login. If both were necessary, an account with one part enabled would
        // never authenticate.
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

            let report = run(&collection, parts, Some("vera@example.com"), false)
                .expect("nothing holds the freeze");

            assert_eq!(report.asked, Asked::Credentials, "{parts:?}");
            assert!(
                collection.calls().contains(&"request_credentials"),
                "{parts:?}"
            );
        }
    });
}

#[test]
fn an_account_that_names_no_user_is_authenticated_without_a_password() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `e_backend_schedule_credentials_required` asks for a password.
        // `e_backend_schedule_authenticate` asks for authentication now, with the
        // available data. `credentials()` reads a source with no user as an
        // anonymous account, which has no password. A prompt would be unnecessary,
        // and `credentials()` would ignore the typed password.
        let collection = Collection::with(vec![]);

        let report = run(&collection, Parts::ALL, None, false).expect("nothing holds the freeze");

        assert_eq!(report.asked, Asked::Anonymously);
        assert_eq!(
            collection.calls(),
            [
                "freeze",
                "chain_up",
                "claim_all_resources",
                "offer_creation",
                "allow_rename",
                "authenticate_anonymously",
                "thaw",
            ]
        );
    });
}

#[test]
fn a_cached_source_this_backend_cannot_name_is_not_exported() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // EDS cannot cause this case, but the behaviour is defined. EDS caches
        // only a source for which `dup_resource_id` of this crate gave an answer.
        // Thus a claimed source that gives `None` changed after the cache. If
        // exported, no resource id could match it again.
        // `e_collection_backend_new_child` finds an existing child through
        // `dup_resource_id`, so each later populate would create it again. The
        // source stays unexported. The count is the only record, because
        // `populate` returns `void`.
        let collection = Collection::with(vec![
            Source::cached(ChildKind::AddressBook, "B1"),
            Source::bare("jmap-cached-nameless"),
        ]);

        let report = run(&collection, Parts::ALL, None, false).expect("nothing holds the freeze");

        assert_eq!(collection.published_ids(), ["addressbook:B1"]);
        assert_eq!(report.children, ["addressbook:B1"]);
        assert_eq!(report.unidentified, 1);
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_populate_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}
