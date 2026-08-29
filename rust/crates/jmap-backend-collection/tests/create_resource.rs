// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Creating a collection on the server, from the scratch `ESource` EDS hands
// `create_resource_sync` — against a real `EServerSideSource` and a real
// `jmap-mockd`.
//
// `jmap-collection-sync`'s `tests/create.rs` covers what the create *asks the
// server*, with no `ESource` in it. This covers the two ends that need the
// headers, and the join between them: the kind and name read off a source
// Evolution's dialog would have built, and the source that same create leaves
// behind — which has to be a source `resource_id_of` reads back as this child,
// because that is what stops the next populate creating a second one for the
// same collection.
//
// The scratch source is built the way EDS builds it, with
// `e_server_side_source_new` on a `GFile` — the same daemon-free half of
// `evolution-source-registry` `tests/recipe.rs` uses. That matters here more
// than it does there: three of the four things `adopt_created` writes are
// `EServerSideSource` properties, and a plain `e_source_new_with_uid` source
// would answer every one of them with a `g_return_if_fail` critical rather than
// with a value.

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use eds_sys::{
    E_SOURCE_EXTENSION_ADDRESS_BOOK, E_SOURCE_EXTENSION_CALENDAR, E_SOURCE_EXTENSION_RESOURCE,
    ESource, ESourceRegistryServer, e_server_side_source_get_write_directory,
    e_server_side_source_new, e_source_address_book_get_type, e_source_calendar_get_type,
    e_source_get_extension, e_source_get_parent, e_source_get_uid, e_source_get_writable,
    e_source_has_extension, e_source_registry_server_new, e_source_set_display_name,
    g_file_new_for_path,
};
use glib_sys::GFALSE;
use gobject_sys::{GObject, g_object_unref};
use jmap_backend_collection::create_resource::{
    adopt_created, create_on_server, requested_of, stored_password_of,
};
use jmap_backend_collection::resource_id::{kind_of, resource_id_of};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::source::ConnectTarget;
use jmap_client::Credentials;
use jmap_collection_sync::child_source::Connection;
use jmap_collection_sync::{Child, ChildKind, Requested};
use jmap_mock::MockServer;

mod common;
use common::{with_timeout, with_timeout_duration};

/// A distinct file name per source, so two sources in one test process never
/// share an `ESource` uid — EDS derives the uid from the file name.
static NEXT: AtomicU32 = AtomicU32::new(0);
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The account this collection belongs to, as `create_resource_sync` reads it
/// off the account `ESource`. Deliberately not the mock's address: the child has
/// to be written from the connection the *account* names, and a test whose two
/// answers were the same string could not tell that from a child written out of
/// whatever URL the create happened to use.
fn connection() -> Connection {
    Connection {
        host: "jmap.example.com".to_owned(),
        port: Some(8443),
        user: Some("vera@example.com".to_owned()),
        auth_method: Some("plain/password".to_owned()),
        secure: true,
    }
}

/// A scratch `EServerSideSource`, as EDS's `remote_create` handler builds one.
///
/// `server_side_source_remote_create_cb` calls `e_server_side_source_new` on a
/// file in the registry's user directory that does not exist yet and then sets
/// the keyfile Evolution sent onto it. This builds the same object and sets the
/// same two things that keyfile carries — the kind extension and the display
/// name — directly, since what is under test is what this backend does with a
/// source in that state, not EDS's keyfile parser.
struct Scratch {
    /// Held for as long as the source: `e_server_side_source_new` keeps only a
    /// weak reference to it, and a source whose server has been finalized is one
    /// every `EServerSideSource` setter refuses.
    server: *mut ESourceRegistryServer,
    source: *mut ESource,
}

impl Scratch {
    fn new(kind: Option<ChildKind>, display_name: &str) -> Self {
        // SAFETY: no arguments; `e_source_get_extension` cannot find an
        // extension class whose type nothing has referenced yet.
        unsafe {
            e_source_address_book_get_type();
            e_source_calendar_get_type();
        }

        let path = std::env::temp_dir().join(format!(
            "jmap-scratch-{}-{}.source",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let path = CString::new(path.into_os_string().into_encoded_bytes())
            .expect("no NUL in a temp path");
        let mut error = ptr::null_mut();

        // SAFETY: the constructor takes no arguments; the path is
        // NUL-terminated and copied; the GFile is owned here and released below.
        let (server, source) = unsafe {
            // `e_source_registry_server_new` is declared as returning its own
            // base class, `EDBusServer *`, the way EDS's own callers use it; the
            // cast back down is the one C does implicitly. Constructing one owns
            // no bus name and reads no sources — that only happens when it runs.
            let server = e_source_registry_server_new().cast::<ESourceRegistryServer>();
            let file = g_file_new_for_path(path.as_ptr());
            let source = e_server_side_source_new(server, file, &mut error);
            g_object_unref(file.cast());
            (server, source)
        };
        assert!(
            !source.is_null(),
            "e_server_side_source_new failed: {}",
            // SAFETY: a NULL return means the GError was set.
            unsafe { CStr::from_ptr((*error).message) }.to_string_lossy()
        );

        if let Some(kind) = kind {
            let extension = match kind {
                ChildKind::AddressBook => E_SOURCE_EXTENSION_ADDRESS_BOOK,
                ChildKind::Calendar => E_SOURCE_EXTENSION_CALENDAR,
            };
            // SAFETY: a live source and a header constant; the extension is
            // created on demand and owned by the source. Wanted here for exactly
            // what it does — this is what makes the scratch source an address
            // book to Evolution's dialog and to `kind_of`.
            unsafe { assert!(!e_source_get_extension(source, extension.as_ptr()).is_null()) };
        }

        let display_name = CString::new(display_name).expect("no NUL in a test name");
        // SAFETY: a live source and a NUL-terminated string the setter copies.
        unsafe { e_source_set_display_name(source, display_name.as_ptr()) };

        Self { server, source }
    }

    fn parent(&self) -> Option<String> {
        // SAFETY: a live source; the getter returns NULL or a string it owns.
        unsafe { read_string(e_source_get_parent(self.source)) }
    }

    fn writable(&self) -> bool {
        // SAFETY: as above.
        unsafe { e_source_get_writable(self.source) != GFALSE }
    }

    fn write_directory(&self) -> Option<String> {
        // SAFETY: as above, and the source is an `EServerSideSource`.
        unsafe { read_string(e_server_side_source_get_write_directory(self.source.cast())) }
    }

    fn has_extension(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a header constant.
        unsafe { e_source_has_extension(self.source, name.as_ptr()) != GFALSE }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // SAFETY: this holds the only reference to each; the source is released
        // first because it holds a weak reference to the server.
        unsafe {
            g_object_unref(self.source.cast());
            g_object_unref(self.server.cast::<GObject>());
        }
    }
}

fn requested_kind(scratch: &Scratch) -> Option<Requested> {
    // SAFETY: a live source this test holds a reference to.
    unsafe { requested_of(scratch.source) }
}

/// The whole create, as the vfunc runs it: the collection made on the mock, then
/// the scratch source finished into the child for it.
fn create(server: &MockServer, scratch: &Scratch) -> Child {
    let requested = requested_kind(scratch).expect("the scratch source names a kind");
    let child = create_on_server(
        &ConnectTarget::Origin(server.origin().to_owned()),
        false,
        Credentials::none(),
        &requested,
    )
    .expect("the mock creates collections");

    // SAFETY: a live `EServerSideSource` this test holds a reference to.
    unsafe {
        adopt_created(
            scratch.source,
            &child,
            &connection(),
            "jmap-test-account",
            Some("/var/lib/jmap-test-cache"),
        )
    }
    .expect("every setting of a child this backend describes is writable");

    child
}

#[test]
fn a_scratch_address_book_asks_for_an_address_book_under_its_display_name() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scratch = Scratch::new(Some(ChildKind::AddressBook), "Work");

        assert_eq!(
            requested_kind(&scratch),
            Some(Requested {
                kind: ChildKind::AddressBook,
                display_name: "Work".to_owned(),
            })
        );
    });
}

#[test]
fn a_scratch_calendar_asks_for_a_calendar_under_its_display_name() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scratch = Scratch::new(Some(ChildKind::Calendar), "Trips");

        assert_eq!(
            requested_kind(&scratch),
            Some(Requested {
                kind: ChildKind::Calendar,
                display_name: "Trips".to_owned(),
            })
        );
    });
}

#[test]
fn a_scratch_source_that_names_no_kind_asks_for_nothing() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // EDS's own documentation of the vfunc: "If this cannot be determined
        // without ambiguity, the function must return an error." Guessing a kind
        // would create the wrong sort of collection under a name the user chose for
        // the other.
        let scratch = Scratch::new(None, "Work");

        assert_eq!(requested_kind(&scratch), None);
    });
}

#[test]
fn reading_the_kind_does_not_give_the_scratch_source_an_extension_it_lacked() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `e_source_get_extension` *creates* what it cannot find, and a scratch
        // source is a real source that gets written to disk — so a read that reached
        // for `[Calendar]` would turn every new address book into a source both
        // factories claim.
        let scratch = Scratch::new(Some(ChildKind::AddressBook), "Work");

        // SAFETY: a live source this test holds a reference to.
        assert_eq!(
            unsafe { kind_of(scratch.source) },
            Some(ChildKind::AddressBook)
        );
        assert!(!scratch.has_extension(E_SOURCE_EXTENSION_CALENDAR));
        assert!(!scratch.has_extension(E_SOURCE_EXTENSION_RESOURCE));
    });
}

#[test]
fn a_created_address_book_leaves_a_source_this_backend_reads_back_as_its_child() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The join, and the property that keeps a create from doubling: EDS pairs a
        // published child with a resource id by asking `dup_resource_id` about it
        // (`collection_backend_ref_child_source`), so a created source whose
        // resource id does not come back is one the next populate creates a *second*
        // source for — one server-side address book, two rows in the sidebar.
        let server = MockServer::builder().start();
        let scratch = Scratch::new(Some(ChildKind::AddressBook), "Work");

        let child = create(&server, &scratch);

        // SAFETY: a live source this test holds a reference to.
        assert_eq!(
            unsafe { resource_id_of(scratch.source) },
            Some(child.resource_id.clone())
        );
        assert_eq!(child.kind, ChildKind::AddressBook);
    });
}

#[test]
fn a_created_calendar_leaves_a_source_this_backend_reads_back_as_its_child() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let server = MockServer::builder().start();
        let scratch = Scratch::new(Some(ChildKind::Calendar), "Trips");

        let child = create(&server, &scratch);

        // SAFETY: a live source this test holds a reference to.
        assert_eq!(
            unsafe { resource_id_of(scratch.source) },
            Some(child.resource_id.clone())
        );
        assert_eq!(child.kind, ChildKind::Calendar);
    });
}

#[test]
fn a_created_child_reaches_the_server_the_account_names() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Written from the *account's* connection, not from wherever the create
        // happened to connect. A child that named the discovery URL would work for
        // exactly as long as the two agreed.
        let server = MockServer::builder().start();
        let scratch = Scratch::new(Some(ChildKind::AddressBook), "Work");

        let child = create(&server, &scratch);

        // Read back the way the address book backend reads it: through
        // `SourceConfig`, not through the setters this test could have called.
        // SAFETY: a live source this test holds a reference to.
        let config =
            unsafe { jmap_backend_core::source::SourceConfig::from_source(scratch.source) }
                .expect("a child this backend wrote is one the book backend can read");
        assert_eq!(
            config.target,
            ConnectTarget::Origin("https://jmap.example.com:8443".to_owned())
        );
        assert_eq!(
            config.resource_id.as_deref(),
            Some(child.collection_id.as_str())
        );
    });
}

#[test]
fn a_created_child_is_parented_writable_and_written_back_to_the_collections_cache() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // The three things `collection_backend_new_source()` sets on a child EDS
        // mints and that a scratch source therefore lacks. Without the parent the
        // source is not a child of this account at all — `child_added` never fires
        // for it; without the write directory its `.source` file stays in the
        // registry's user directory, where removing the account leaves it behind;
        // without `writable` the user cannot rename the address book they just made.
        let server = MockServer::builder().start();
        let scratch = Scratch::new(Some(ChildKind::AddressBook), "Work");

        create(&server, &scratch);

        assert_eq!(scratch.parent().as_deref(), Some("jmap-test-account"));
        assert!(scratch.writable());
        assert_eq!(
            scratch.write_directory().as_deref(),
            Some("/var/lib/jmap-test-cache")
        );
    });
}

#[test]
fn adopting_a_created_child_is_not_what_offers_it_for_deletion() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Deliberate, and where it would be noticed if it changed: evolution-ews
        // sets `remote-deletable` at each of the three sites that mint a child, this
        // one included, while this backend sets it in one place —
        // `delete_resource::offer_deletion`, from the `child_added` that fires when
        // this source is published a moment later. So a created child *is* offered
        // for deletion in Evolution; what this pins is that adopting one is not the
        // place that decides it, because a second site here could drift from the
        // funnel and offer "Delete" on something the funnel would have refused.
        let server = MockServer::builder().start();
        let scratch = Scratch::new(Some(ChildKind::AddressBook), "Work");

        create(&server, &scratch);

        // SAFETY: a live source this test holds a reference to.
        assert!(
            unsafe { eds_sys::e_source_get_remote_deletable(scratch.source) } == GFALSE,
            "adopt_created set remote-deletable; the flag has two homes now"
        );
    });
}

/// Records every field of every event it sees, as `(name, value)` pairs —
/// mirrors `jmap_backend_core::trampoline`'s own test-only subscriber, which
/// this crate cannot reuse directly (it is private to that crate) and has no
/// `tracing-subscriber` dev-dependency to build one from instead.
struct CapturingSubscriber {
    captured: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
}

struct Recorder<'a>(&'a std::sync::Mutex<Vec<(String, String)>>);

impl tracing::field::Visit for Recorder<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0
            .lock()
            .unwrap()
            .push((field.name().to_owned(), format!("{value:?}")));
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0
            .lock()
            .unwrap()
            .push((field.name().to_owned(), value.to_owned()));
    }
}

impl tracing::Subscriber for CapturingSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        event.record(&mut Recorder(&self.captured));
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

#[test]
fn stored_password_of_a_gone_registry_server_names_the_account_in_a_structured_field() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // `server.is_null()` is the "registry server is gone" branch — reachable
        // with no `ESourceRegistryServer` at all, which keeps this test to the one
        // thing under test (the `account_id` field) rather than also standing up a
        // credentials provider that then has to fail a lookup.
        let scratch = Scratch::new(Some(ChildKind::AddressBook), "Work");
        // SAFETY: a live source this test holds a reference to; the uid comes back
        // `(transfer none)`.
        let account_id = unsafe { read_string(e_source_get_uid(scratch.source)) }
            .expect("every source has a uid");

        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let subscriber = CapturingSubscriber {
            captured: captured.clone(),
        };

        tracing::subscriber::with_default(subscriber, || {
            // SAFETY: a NULL server (the branch under test), a live source, and no
            // cancellable — all valid per `stored_password_of`'s own contract.
            let password = unsafe {
                stored_password_of(
                    ptr::null_mut(),
                    scratch.source,
                    ptr::null_mut(),
                    "create_resource_sync",
                )
            };
            assert_eq!(password, None);
        });

        assert!(
            captured
                .lock()
                .unwrap()
                .contains(&("account_id".to_owned(), account_id)),
            "expected an account_id field naming the source's own uid, got {:?}",
            captured.lock().unwrap()
        );
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_create_resource_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}
