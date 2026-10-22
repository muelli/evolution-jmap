// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests for the read of the resource id from a child source. The tests use
// real `ESource`s that the EDS setters build, not a struct made by hand.
//
// In this vfunc, an incorrect answer does not cause a failed operation. It
// causes data loss. `collection_backend_load_resources()` deletes the cache
// file of each child that answers `NULL`. It also deletes the cache file of
// each child whose answer is the same as the answer of an earlier child. Each
// case below is a way for this backend to lose offline data of the user.

use std::ffi::{CString, c_void};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use eds_sys::{
    E_SOURCE_EXTENSION_ADDRESS_BOOK, E_SOURCE_EXTENSION_CALENDAR, E_SOURCE_EXTENSION_RESOURCE,
    ESource, ESourceResource, e_source_address_book_get_type, e_source_calendar_get_type,
    e_source_get_extension, e_source_has_extension, e_source_new_with_uid,
    e_source_resource_set_identity,
};
use glib_sys::{
    G_LOG_LEVEL_CRITICAL, G_LOG_LEVEL_WARNING, GFALSE, GLogLevelFlags, g_log_remove_handler,
    g_log_set_handler, gchar,
};
use gobject_sys::g_object_unref;
use jmap_backend_collection::resource_id::{KIND_EXTENSIONS, kind_of, resource_id_of};
use jmap_collection_sync::child_source::{Connection, EXTENSION_ADDRESS_BOOK, EXTENSION_CALENDAR};
use jmap_collection_sync::{Child, ChildKind};
use jmap_proto::Id;

mod common;
use common::{with_timeout, with_timeout_duration};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// An `ESource` without the registry, made with `e_source_new_with_uid`. The
/// extension functions operate on it as they do in a backend.
struct TestSource(*mut ESource);

impl TestSource {
    fn new() -> Self {
        // `e_source_get_extension` walks the children of
        // `E_TYPE_SOURCE_EXTENSION` to find an extension class. It cannot find a
        // type that is not yet referenced. In EDS, the libraries register their
        // types before the first source exists. In this test binary, the calls
        // to the two type functions do that registration.
        // SAFETY: these functions take no arguments, and the type system
        // initialises itself.
        unsafe {
            e_source_address_book_get_type();
            e_source_calendar_get_type();
        }

        let uid = CString::new("jmap-collection-child").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: the documented arguments are a NUL-terminated uid, the default
        // main context and a GError out-parameter.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    /// Gives the source the extension named by `group`. The extension makes the
    /// source an address book or a calendar. A `.source` file has it as a
    /// keyfile group.
    fn with_extension(self, group: &str) -> Self {
        let group = CString::new(group).expect("no NUL in an extension name");
        // SAFETY: a live source and a NUL-terminated name. The source creates
        // the extension on demand and owns it.
        let extension = unsafe { e_source_get_extension(self.0, group.as_ptr()) };
        assert!(!extension.is_null(), "EDS knows no such extension");
        self
    }

    fn with_identity(self, identity: &str) -> Self {
        let identity = CString::new(identity).expect("no NUL in a test identity");
        // SAFETY: as above, and the setter copies the string.
        unsafe {
            let resource: *mut ESourceResource =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_RESOURCE.as_ptr()).cast();
            e_source_resource_set_identity(resource, identity.as_ptr());
        }
        self
    }

    fn has_extension(&self, name: &std::ffi::CStr) -> bool {
        // SAFETY: a live source and a NUL-terminated name.
        unsafe { e_source_has_extension(self.0, name.as_ptr()) != GFALSE }
    }

    fn resource_id(&self) -> Option<String> {
        // SAFETY: a live source.
        unsafe { resource_id_of(self.0) }
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: this releases the reference from `e_source_new_with_uid`
        // exactly once.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

fn connection() -> Connection {
    Connection {
        host: "jmap.example.com".to_owned(),
        port: None,
        user: None,
        auth_method: None,
        secure: true,
    }
}

fn child(kind: ChildKind, collection: &str) -> Child {
    Child {
        resource_id: kind.resource_id(&Id::new(collection)),
        kind,
        display_name: "Whatever".to_owned(),
        account_id: Id::new("A1"),
        collection_id: Id::new(collection),
        is_default: false,
        color: None,
        read_only: false,
        remote_deletable: true,
    }
}

#[test]
fn the_extension_names_are_the_ones_eds_defines() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `jmap-collection-sync` has the names as string literals, because it
        // must build without the EDS headers. Each name is the argument to
        // `e_source_get_extension` and a group of a `.source` file. A spelling
        // error does not cause an error. It causes a second, empty extension
        // that nothing reads. Only this crate has both spellings in scope, so
        // only this crate can compare them.
        assert_eq!(
            E_SOURCE_EXTENSION_ADDRESS_BOOK.to_str(),
            Ok(EXTENSION_ADDRESS_BOOK)
        );
        assert_eq!(E_SOURCE_EXTENSION_CALENDAR.to_str(), Ok(EXTENSION_CALENDAR));

        // Each pair in the dispatch table of this crate must also agree. Thus a
        // new kind cannot add a third spelling.
        assert_eq!(
            KIND_EXTENSIONS
                .iter()
                .map(|(defined, ours)| (defined.to_str().expect("ASCII"), *ours))
                .collect::<Vec<_>>(),
            [
                (EXTENSION_ADDRESS_BOOK, EXTENSION_ADDRESS_BOOK),
                (EXTENSION_CALENDAR, EXTENSION_CALENDAR),
            ]
        );
    });
}

#[test]
fn an_address_book_child_is_read_back_as_the_resource_id_it_was_created_under() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let source = TestSource::new()
            .with_extension(EXTENSION_ADDRESS_BOOK)
            .with_identity("AB1");

        assert_eq!(source.resource_id().as_deref(), Some("addressbook:AB1"));
    });
}

#[test]
fn a_calendar_child_is_read_back_as_a_calendar() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let source = TestSource::new()
            .with_extension(EXTENSION_CALENDAR)
            .with_identity("Cal1");

        assert_eq!(source.resource_id().as_deref(), Some("calendar:Cal1"));
    });
}

#[test]
fn the_two_kinds_of_child_of_one_jmap_id_do_not_share_a_resource_id() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // This is why this backend overrides the vfunc. The EDS implementation
        // returns `[Resource] Identity` without change. A JMAP account can give
        // the same id to an address book and a calendar. RFC 8620 ids are unique
        // for each data type, not for each account. If two children have one
        // resource id, `collection_backend_load_resources()` keeps the first. It
        // deletes the cache file of the second as redundant.
        let book = TestSource::new()
            .with_extension(EXTENSION_ADDRESS_BOOK)
            .with_identity("X1");
        let calendar = TestSource::new()
            .with_extension(EXTENSION_CALENDAR)
            .with_identity("X1");

        assert_ne!(book.resource_id(), calendar.resource_id());
        assert!(book.resource_id().is_some());
        assert!(calendar.resource_id().is_some());
    });
}

#[test]
fn every_child_this_backend_describes_reads_back_as_itself() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // This round trip must be correct for each child that `populate`
        // creates. This function must read what `Child::settings` writes.
        // Otherwise, the next start deletes the child from the cache.
        for (kind, collection) in [
            (ChildKind::AddressBook, "AB1"),
            (ChildKind::Calendar, "Cal1"),
            (ChildKind::AddressBook, "Shared"),
            (ChildKind::Calendar, "Shared"),
        ] {
            let child = child(kind, collection);
            let settings = child.settings(&connection());
            let identity = settings
                .iter()
                .find(|setting| (setting.group, setting.key) == ("Resource", "Identity"))
                .expect("every child is written with an identity");

            let source = TestSource::new()
                .with_extension(kind.extension())
                .with_identity(&identity.value);

            assert_eq!(source.resource_id().as_deref(), Some(&*child.resource_id));
        }
    });
}

#[test]
fn a_source_of_no_kind_this_backend_creates_is_not_claimed() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // EDS calls `dup_resource_id` for each `.source` in the cache directory
        // of the backend. A claim on a source that is not ours puts it in the
        // unclaimed-resources table of EDS. A real child can have the same name.
        // EDS then deletes one of the two.
        let source = TestSource::new()
            .with_extension("Mail Account")
            .with_identity("A1");

        assert_eq!(source.resource_id(), None);
    });
}

#[test]
fn a_child_with_no_identity_is_not_claimed_and_is_not_given_one() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // This test examines two things. The second is less clear:
        // `e_source_get_extension` *creates* the extension that it gets. A read of
        // the identity with it adds an empty `[Resource]` group to each source
        // that this vfunc gets, also to sources of other backends. Thus, examine
        // the presence of the extension before the read.
        let source = TestSource::new().with_extension(EXTENSION_ADDRESS_BOOK);

        assert_eq!(source.resource_id(), None);
        assert!(
            !source.has_extension(E_SOURCE_EXTENSION_RESOURCE),
            "reading the identity added a [Resource] extension to a source that had none"
        );
    });
}

#[test]
fn an_empty_identity_is_no_identity() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // A `.source` with `Identity=` gives the empty string. An empty JMAP id
        // is not an id. All such children would share the answer
        // `"addressbook:"`.
        let source = TestSource::new()
            .with_extension(EXTENSION_ADDRESS_BOOK)
            .with_identity("");

        assert_eq!(source.resource_id(), None);
    });
}

unsafe extern "C" fn count_message(
    _domain: *const gchar,
    _level: GLogLevelFlags,
    _message: *const gchar,
    counter: *mut c_void,
) {
    // SAFETY: the test gives the address of an `AtomicUsize` that outlives
    // the handler.
    unsafe { &*counter.cast::<AtomicUsize>() }.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn a_null_source_is_not_claimed_rather_than_dereferenced() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // EDS does not pass NULL. The vfunc is a C entry point, and a NULL check
        // is the easiest protection against an incorrect caller. Without the
        // check, `e_source_has_extension` also refuses NULL, but it logs an
        // `e-data-server` critical. `G_DEBUG=fatal-criticals` makes that abort.
        let criticals = AtomicUsize::new(0);
        // SAFETY: the domain is a 'static literal. The handler reads
        // `criticals` only until the removal of the handler below.
        let handler = unsafe {
            g_log_set_handler(
                c"e-data-server".as_ptr(),
                G_LOG_LEVEL_CRITICAL | G_LOG_LEVEL_WARNING,
                Some(count_message),
                ptr::from_ref(&criticals).cast_mut().cast(),
            )
        };
        // SAFETY: NULL is an explicitly permitted argument.
        let (resource_id, kind) =
            unsafe { (resource_id_of(ptr::null_mut()), kind_of(ptr::null_mut())) };
        // SAFETY: the ID is from the call above, and the removal happens once.
        unsafe { g_log_remove_handler(c"e-data-server".as_ptr(), handler) };

        assert_eq!(resource_id, None);
        assert_eq!(kind, None);
        assert_eq!(
            criticals.load(Ordering::SeqCst),
            0,
            "a NULL source reached EDS, which logged a critical"
        );
    });
}

#[test]
fn a_source_with_both_kinds_is_read_as_an_address_book() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // This backend never writes such a source, but a user can edit one. The
        // order of `KIND_EXTENSIONS` decides its resource id. A change of the
        // order changes the id of an existing cache file, and EDS then deletes it.
        let source = TestSource::new()
            .with_extension(EXTENSION_CALENDAR)
            .with_extension(EXTENSION_ADDRESS_BOOK)
            .with_identity("X1");

        assert_eq!(source.resource_id().as_deref(), Some("addressbook:X1"));
        // SAFETY: a live source.
        assert_eq!(unsafe { kind_of(source.0) }, Some(ChildKind::AddressBook));
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_resource_id_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}
