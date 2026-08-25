// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Structured tracing tests for `jmap-backend-collection` — Track B1 follow-up.
//!
//! Asserts structured fields attached to `tracing` events during collection
//! authentication, fan-out discovery, resource creation, resource deletion,
//! cache population, and password resolution.

use std::cell::Cell;
use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use eds_sys::{
    E_SOURCE_AUTHENTICATION_ACCEPTED, E_SOURCE_AUTHENTICATION_REQUIRED,
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_COLLECTION, E_SOURCE_EXTENSION_SECURITY,
    ESource, ESourceAuthentication, ESourceCollection, ESourceRegistryServer, ESourceSecurity,
    e_server_side_source_new, e_source_address_book_get_type, e_source_authentication_get_type,
    e_source_authentication_set_host, e_source_authentication_set_port,
    e_source_authentication_set_user, e_source_calendar_get_type, e_source_collection_get_type,
    e_source_collection_set_calendar_enabled, e_source_collection_set_contacts_enabled,
    e_source_collection_set_mail_enabled, e_source_get_extension, e_source_new_with_uid,
    e_source_registry_server_new, e_source_security_get_type, e_source_security_set_secure,
    e_source_set_enabled, g_file_new_for_path,
};
use glib_sys::{GFALSE, GTRUE};
use gobject_sys::{g_object_ref, g_object_unref};
use jmap_backend_collection::authenticate::{Login, authenticate_with};
use jmap_backend_collection::collection_source::Server;
use jmap_backend_collection::create_resource::{
    adopt_created, create_on_server, stored_password_of,
};
use jmap_backend_collection::delete_resource::{delete_on_server, offer_deletion};
use jmap_backend_collection::fan_out::{Collection as CollectionTrait, fan_out};
use jmap_backend_collection::populate::{Populating, populate};
use jmap_backend_collection::removal::remove_obsolete;
use jmap_backend_core::source::ConnectTarget;
use jmap_client::Credentials;
use jmap_collection_sync::child_source::Connection;
use jmap_collection_sync::{Child, ChildKind, CollectionLayout, Doomed, Fanout, Parts, Requested};
use jmap_mock::MockServer;
use jmap_proto::Id;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id as SpanId, Record};
use tracing::{Event, Level, Metadata, Subscriber};

static NEXT: AtomicU32 = AtomicU32::new(0);

struct CapturingSubscriber {
    captured: Arc<Mutex<Vec<(Level, String, String)>>>,
}

struct Recorder<'a> {
    level: Level,
    sink: &'a Mutex<Vec<(Level, String, String)>>,
}

impl Visit for Recorder<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.sink
            .lock()
            .unwrap()
            .push((self.level, field.name().to_owned(), format!("{value:?}")));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.sink
            .lock()
            .unwrap()
            .push((self.level, field.name().to_owned(), value.to_owned()));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.sink
            .lock()
            .unwrap()
            .push((self.level, field.name().to_owned(), value.to_string()));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.sink
            .lock()
            .unwrap()
            .push((self.level, field.name().to_owned(), value.to_string()));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.sink
            .lock()
            .unwrap()
            .push((self.level, field.name().to_owned(), value.to_string()));
    }
}

impl Subscriber for CapturingSubscriber {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> SpanId {
        SpanId::from_u64(1)
    }

    fn record(&self, _span: &SpanId, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &SpanId, _follows: &SpanId) {}

    fn event(&self, event: &Event<'_>) {
        event.record(&mut Recorder {
            level: *event.metadata().level(),
            sink: &self.captured,
        });
    }

    fn enter(&self, _span: &SpanId) {}

    fn exit(&self, _span: &SpanId) {}
}

mod common;
use common::{with_timeout, with_timeout_duration};

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn capture(run: impl FnOnce()) -> Vec<(Level, String, String)> {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let subscriber = CapturingSubscriber {
        captured: captured.clone(),
    };
    tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        run();
    });
    std::mem::take(&mut *captured.lock().unwrap())
}

fn untraced<T>(run: impl FnOnce() -> T) -> T {
    let subscriber = CapturingSubscriber {
        captured: Arc::new(Mutex::new(Vec::new())),
    };
    tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        run()
    })
}

fn has(captured: &[(Level, String, String)], level: Level, name: &str, value: &str) -> bool {
    captured
        .iter()
        .any(|(l, n, v)| *l == level && n == name && v == value)
}

fn connection() -> Connection {
    Connection {
        host: "jmap.example.com".to_owned(),
        port: Some(8443),
        user: Some("vera@example.com".to_owned()),
        auth_method: Some("plain/password".to_owned()),
        secure: true,
    }
}

struct Source(*mut ESource);

impl Source {
    fn new(uid: &str) -> Self {
        let uid = CString::new(uid).expect("no NUL in a test uid");
        let mut error = ptr::null_mut();
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    fn dup(&self) -> *mut ESource {
        unsafe { g_object_ref(self.0.cast()) }.cast()
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        unsafe { g_object_unref(self.0.cast()) };
    }
}

struct TestSource(*mut ESource);

impl TestSource {
    fn new(uid: &str) -> Self {
        unsafe {
            e_source_collection_get_type();
            e_source_authentication_get_type();
            e_source_security_get_type();
        }
        let uid = CString::new(uid).expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        unsafe { e_source_set_enabled(source, GTRUE) };
        Self(source)
    }

    fn parts(self, parts: Parts) -> Self {
        unsafe {
            let collection: *mut ESourceCollection =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_COLLECTION.as_ptr()).cast();
            let flag = |on: bool| if on { GTRUE } else { GFALSE };
            e_source_collection_set_mail_enabled(collection, flag(parts.mail));
            e_source_collection_set_contacts_enabled(collection, flag(parts.contacts));
            e_source_collection_set_calendar_enabled(collection, flag(parts.calendars));
        }
        self
    }

    fn authentication(self, host: &str, port: u16, user: Option<&str>) -> Self {
        let host = CString::new(host).expect("no NUL in a test host");
        let user = user.map(|user| CString::new(user).expect("no NUL in a test user"));
        unsafe {
            let auth: *mut ESourceAuthentication =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast();
            e_source_authentication_set_host(auth, host.as_ptr());
            e_source_authentication_set_port(auth, port);
            if let Some(user) = &user {
                e_source_authentication_set_user(auth, user.as_ptr());
            }
        }
        self
    }

    fn secure(self, secure: bool) -> Self {
        unsafe {
            let security: *mut ESourceSecurity =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_SECURITY.as_ptr()).cast();
            e_source_security_set_secure(security, if secure { GTRUE } else { GFALSE });
        }
        self
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        unsafe { g_object_unref(self.0.cast()) };
    }
}

struct Scratch {
    server: *mut ESourceRegistryServer,
    source: *mut ESource,
}

impl Scratch {
    fn new() -> Self {
        unsafe {
            e_source_address_book_get_type();
            e_source_calendar_get_type();
        }
        let path = std::env::temp_dir().join(format!(
            "jmap-scratch-tracing-{}-{}.source",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let path = CString::new(path.into_os_string().into_encoded_bytes())
            .expect("no NUL in a temp path");
        let mut error = ptr::null_mut();
        let (server, source) = unsafe {
            let server = e_source_registry_server_new().cast::<ESourceRegistryServer>();
            let file = g_file_new_for_path(path.as_ptr());
            let source = e_server_side_source_new(server, file, &mut error);
            g_object_unref(file.cast());
            (server, source)
        };
        assert!(
            !source.is_null(),
            "e_server_side_source_new failed: {}",
            unsafe { CStr::from_ptr((*error).message) }.to_string_lossy()
        );
        Self { server, source }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        unsafe {
            g_object_unref(self.source.cast());
            g_object_unref(self.server.cast());
        }
    }
}

#[derive(Default)]
struct MockCollection {
    existing: Vec<Source>,
    created: RefCell<Vec<(String, Source)>>,
    published: RefCell<Vec<*mut ESource>>,
}

unsafe impl CollectionTrait for MockCollection {
    fn existing_children(&self) -> Vec<*mut ESource> {
        self.existing.iter().map(Source::dup).collect()
    }

    fn new_child(&self, resource_id: &str) -> *mut ESource {
        let child = Source::new(&format!("child-{resource_id}"));
        let ptr = child.dup();
        self.created
            .borrow_mut()
            .push((resource_id.to_owned(), child));
        ptr
    }

    fn is_new_child(&self, child: *mut ESource) -> bool {
        self.created
            .borrow()
            .iter()
            .any(|(_, source)| source.0 == child)
    }

    fn publish(&self, child: *mut ESource) {
        self.published.borrow_mut().push(child);
    }
}

#[derive(Default)]
struct MockPopulating {
    freeze_count: Cell<i32>,
    resources: RefCell<Vec<Source>>,
    published: RefCell<Vec<*mut ESource>>,
    credentials_requested: RefCell<bool>,
    creatable: RefCell<Option<bool>>,
}

unsafe impl Populating for MockPopulating {
    fn freeze(&self) -> bool {
        let before = self.freeze_count.get();
        self.freeze_count.set(before + 1);
        before == 0
    }

    fn thaw(&self) {
        self.freeze_count.set(self.freeze_count.get() - 1);
    }

    fn chain_up(&self) {}

    fn claim_all_resources(&self) -> Vec<*mut ESource> {
        self.resources
            .borrow_mut()
            .drain(..)
            .map(|s| s.dup())
            .collect()
    }

    fn publish(&self, child: *mut ESource) {
        self.published.borrow_mut().push(child);
    }

    fn request_credentials(&self) {
        *self.credentials_requested.borrow_mut() = true;
    }

    fn authenticate_anonymously(&self) {}

    fn offer_creation(&self, creatable: bool) {
        *self.creatable.borrow_mut() = Some(creatable);
    }
}

#[test]
fn authenticate_with_traces_structured_fields() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let source = TestSource::new("acc-test-auth")
            .parts(Parts {
                mail: false,
                contacts: true,
                calendars: false,
            })
            .authentication("127.0.0.1", 8080, Some("vera@example.com"))
            .secure(false);

        let mut error = ptr::null_mut();
        let captured = capture(|| {
            let result = unsafe {
                authenticate_with(
                    source.0,
                    ptr::null(),
                    ptr::null_mut(),
                    &mut error,
                    |_login| Ok(()),
                    |_creds| {},
                )
            };
            assert_eq!(result, E_SOURCE_AUTHENTICATION_REQUIRED);
        });

        assert!(
            has(&captured, Level::DEBUG, "account_id", "acc-test-auth"),
            "expected account_id in authenticate_with, got {captured:?}"
        );
        assert!(
            has(&captured, Level::DEBUG, "uses_oauth2", "false"),
            "expected uses_oauth2 in authenticate_with, got {captured:?}"
        );
    });
}

#[test]
fn authenticate_with_no_parts_traces_fast_path() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let source = TestSource::new("acc-empty-parts").parts(Parts::NONE);

        let mut error = ptr::null_mut();
        let captured = capture(|| {
            let result = unsafe {
                authenticate_with(
                    source.0,
                    ptr::null(),
                    ptr::null_mut(),
                    &mut error,
                    |_login| Ok(()),
                    |_creds| {},
                )
            };
            assert_eq!(result, E_SOURCE_AUTHENTICATION_ACCEPTED);
        });

        assert!(
            has(&captured, Level::DEBUG, "account_id", "acc-empty-parts"),
            "expected account_id in authenticate_with fast path, got {captured:?}"
        );
    });
}

#[test]
fn create_on_server_traces_structured_fields() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let server = MockServer::builder().start();
        let account_id = server.account_id().to_string();
        let target = ConnectTarget::Origin(server.origin().to_owned());
        let requested = Requested {
            kind: ChildKind::AddressBook,
            display_name: "Personal Contacts".to_owned(),
        };

        let captured = capture(|| {
            let _ = create_on_server(&target, Credentials::none(), &requested).expect("created");
        });

        assert!(
            has(&captured, Level::DEBUG, "account_id", &account_id),
            "expected account_id in create_on_server, got {captured:?}"
        );
        assert!(
            has(&captured, Level::DEBUG, "kind", "AddressBook"),
            "expected kind in create_on_server, got {captured:?}"
        );
        assert!(
            has(&captured, Level::DEBUG, "display_name", "Personal Contacts"),
            "expected display_name in create_on_server, got {captured:?}"
        );
        assert!(
            has(&captured, Level::DEBUG, "resource_id", "addressbook:AB1"),
            "expected resource_id in create_on_server, got {captured:?}"
        );
    });
}

#[test]
fn delete_on_server_traces_structured_fields() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let server = MockServer::builder().start();
        let account_id = server.account_id().to_string();
        let target = ConnectTarget::Origin(server.origin().to_owned());
        let requested = Requested {
            kind: ChildKind::AddressBook,
            display_name: "To Delete".to_owned(),
        };

        let created = untraced(|| {
            create_on_server(&target, Credentials::none(), &requested).expect("created")
        });

        let doomed = Doomed {
            kind: ChildKind::AddressBook,
            collection_id: created.collection_id.clone(),
        };

        let captured = capture(|| {
            delete_on_server(&target, Credentials::none(), &doomed).expect("deleted");
        });

        assert!(
            has(&captured, Level::DEBUG, "account_id", &account_id),
            "expected account_id in delete_on_server, got {captured:?}"
        );
        assert!(
            has(
                &captured,
                Level::DEBUG,
                "collection_id",
                created.collection_id.as_str()
            ),
            "expected collection_id in delete_on_server, got {captured:?}"
        );
        assert!(
            has(&captured, Level::DEBUG, "kind", "AddressBook"),
            "expected kind in delete_on_server, got {captured:?}"
        );
    });
}

#[test]
fn fan_out_traces_structured_fields() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let server = MockServer::builder().start();
        let login = Login {
            server: Server {
                target: ConnectTarget::Origin(server.origin().to_owned()),
                connection: connection(),
            },
            parts: Parts::ALL,
            credentials: Credentials::none(),
        };

        let collection = MockCollection::default();

        let captured = capture(|| {
            let _ = unsafe { fan_out(&collection, &login) }.expect("fanned out");
        });

        assert!(
            has(&captured, Level::DEBUG, "address_books_count", "0"),
            "expected address_books_count in fan_out, got {captured:?}"
        );
        assert!(
            has(&captured, Level::DEBUG, "calendars_count", "0"),
            "expected calendars_count in fan_out, got {captured:?}"
        );
    });
}

#[test]
fn adopt_created_traces_structured_fields() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scratch = Scratch::new();
        let child = Child {
            resource_id: "addressbook-1".to_owned(),
            kind: ChildKind::AddressBook,
            display_name: "Work".to_owned(),
            account_id: Id::new("A1"),
            collection_id: Id::new("1"),
            is_default: false,
            color: None,
            read_only: false,
        };
        let conn = connection();

        let captured = capture(|| unsafe {
            adopt_created(scratch.source, &child, &conn, "account-123", None).expect("adopted");
        });

        assert!(
            has(&captured, Level::DEBUG, "account_uid", "account-123"),
            "expected account_uid in adopt_created, got {captured:?}"
        );
        assert!(
            has(&captured, Level::DEBUG, "resource_id", "addressbook-1"),
            "expected resource_id in adopt_created, got {captured:?}"
        );
        assert!(
            has(&captured, Level::DEBUG, "kind", "AddressBook"),
            "expected kind in adopt_created, got {captured:?}"
        );
    });
}

#[test]
fn offer_deletion_traces_remote_deletable() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let source = Source::new("regular-source");

        let captured = capture(|| {
            let offered = unsafe { offer_deletion(source.0) };
            assert!(!offered);
        });

        assert!(
            has(&captured, Level::DEBUG, "remote_deletable", "false"),
            "expected remote_deletable in offer_deletion, got {captured:?}"
        );
    });
}

#[test]
fn stored_password_of_traces_structured_fields() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let source = Source::new("account-uid-xyz");

        let captured = capture(|| {
            let password = unsafe {
                stored_password_of(ptr::null_mut(), source.0, ptr::null_mut(), "test_context")
            };
            assert!(password.is_none());
        });

        assert!(
            has(&captured, Level::DEBUG, "account_id", "account-uid-xyz"),
            "expected account_id in stored_password_of, got {captured:?}"
        );
        assert!(
            has(&captured, Level::DEBUG, "has_server", "false"),
            "expected has_server in stored_password_of, got {captured:?}"
        );
        assert!(
            has(&captured, Level::DEBUG, "context", "test_context"),
            "expected context in stored_password_of, got {captured:?}"
        );
    });
}

#[test]
fn populate_traces_structured_fields() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let populating = MockPopulating::default();

        let captured = capture(|| {
            let restored = unsafe { populate(&populating, Parts::ALL, Some("vera@example.com")) };
            assert!(restored.is_some());
        });

        assert!(
            has(&captured, Level::DEBUG, "contacts_wanted", "true"),
            "expected contacts_wanted in populate, got {captured:?}"
        );
        assert!(
            has(&captured, Level::DEBUG, "calendars_wanted", "true"),
            "expected calendars_wanted in populate, got {captured:?}"
        );
        assert!(
            has(&captured, Level::DEBUG, "cached_count", "0"),
            "expected cached_count in populate, got {captured:?}"
        );
        assert!(
            has(&captured, Level::DEBUG, "unidentified_count", "0"),
            "expected unidentified_count in populate, got {captured:?}"
        );
        assert!(
            has(&captured, Level::DEBUG, "asked_auth", "Credentials"),
            "expected asked_auth in populate, got {captured:?}"
        );
    });
}

#[test]
fn remove_obsolete_traces_obsolete_count() {
    with_timeout(|| {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let fanout = Fanout {
            parts: Parts::ALL,
            layout: CollectionLayout {
                mail: None,
                contacts: None,
                calendars: None,
            },
            address_books: Vec::new(),
            calendars: Vec::new(),
        };
        let sources: Vec<*mut ESource> = Vec::new();

        let captured = capture(|| {
            let not_removed = unsafe { remove_obsolete(&fanout, &sources) };
            assert!(not_removed.is_empty());
        });

        assert!(
            has(&captured, Level::DEBUG, "obsolete_count", "0"),
            "expected obsolete_count in remove_obsolete, got {captured:?}"
        );
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_tracing_writes_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}
