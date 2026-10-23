// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests for the read of the collection source (the account), with real
// `ESource`s made with the EDS setters, as `tests/resource_id.rs` does for a
// child.
//
// This is all that `populate` knows before it contacts a server: the enabled
// parts, the server, and the data that each child must copy to reach that
// server. Three failures here are silent. A disabled part read as enabled
// gets data that the user does not want. A wrong `[Security]` read changes the
// scheme of all children of the account. An invalid host gives a URL to a
// different place.

use std::ffi::{CStr, CString};

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_COLLECTION, E_SOURCE_EXTENSION_SECURITY,
    ESource, ESourceAuthentication, ESourceCollection, ESourceSecurity,
    e_source_authentication_get_type, e_source_authentication_set_host,
    e_source_authentication_set_method, e_source_authentication_set_port,
    e_source_authentication_set_user, e_source_collection_get_type,
    e_source_collection_set_calendar_enabled, e_source_collection_set_contacts_enabled,
    e_source_collection_set_mail_enabled, e_source_get_extension, e_source_has_extension,
    e_source_security_get_type, e_source_security_set_secure, e_source_set_enabled,
};
use glib_sys::{GFALSE, GTRUE};
use jmap_backend_collection::collection_source::{Server, parts_of, server_of, user_of};
use jmap_backend_core::source::OwnedSource;
use jmap_backend_core::source::{ConnectTarget, SourceError};
use jmap_collection_sync::child_source::Connection;
use jmap_collection_sync::{Child, ChildKind, Parts};
use jmap_proto::Id;

mod common;
use common::{long_values, read_while_another_thread_writes, with_timeout, with_timeout_duration};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// An `ESource` without a registry, as in `tests/resource_id.rs`. EDS uses
/// `e_source_new_with_uid` with a NULL D-Bus object for a source from a keyfile.
struct TestSource(OwnedSource);

impl TestSource {
    fn new() -> Self {
        // `e_source_get_extension` searches the registered children of
        // `E_TYPE_SOURCE_EXTENSION`, so it cannot find an unregistered type. EDS
        // loads the libraries before a source exists. In a test binary, a call
        // to each type accessor does the registration.
        // SAFETY: no arguments, and the type system initialises itself.
        unsafe {
            e_source_collection_get_type();
            e_source_authentication_get_type();
            e_source_security_get_type();
        }

        Self(OwnedSource::new("jmap-collection"))
    }

    /// The `enabled` property of the collection source.
    /// `e_collection_backend_get_part_enabled()` checks it first.
    fn enabled(self, enabled: bool) -> Self {
        // SAFETY: a live source.
        unsafe { e_source_set_enabled(self.0.as_ptr(), if enabled { GTRUE } else { GFALSE }) };
        self
    }

    /// The three part flags of the `[Collection]` extension.
    fn parts(self, parts: Parts) -> Self {
        // SAFETY: a live source and a header constant. The source creates the
        // extension when necessary and owns it.
        unsafe {
            let collection: *mut ESourceCollection =
                e_source_get_extension(self.0.as_ptr(), E_SOURCE_EXTENSION_COLLECTION.as_ptr())
                    .cast();
            let flag = |on: bool| if on { GTRUE } else { GFALSE };
            e_source_collection_set_mail_enabled(collection, flag(parts.mail));
            e_source_collection_set_contacts_enabled(collection, flag(parts.contacts));
            e_source_collection_set_calendar_enabled(collection, flag(parts.calendars));
        }
        self
    }

    fn authentication(
        self,
        host: &str,
        port: u16,
        user: Option<&str>,
        method: Option<&str>,
    ) -> Self {
        let host = CString::new(host).expect("no NUL in a test host");
        let user = user.map(|user| CString::new(user).expect("no NUL in a test user"));
        let method = method.map(|method| CString::new(method).expect("no NUL in a test method"));
        // SAFETY: as above. Each setter copies the string that it receives.
        unsafe {
            let auth: *mut ESourceAuthentication =
                e_source_get_extension(self.0.as_ptr(), E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr())
                    .cast();
            e_source_authentication_set_host(auth, host.as_ptr());
            e_source_authentication_set_port(auth, port);
            if let Some(user) = &user {
                e_source_authentication_set_user(auth, user.as_ptr());
            }
            if let Some(method) = &method {
                e_source_authentication_set_method(auth, method.as_ptr());
            }
        }
        self
    }

    fn secure(self, secure: bool) -> Self {
        // SAFETY: as above.
        unsafe {
            let security: *mut ESourceSecurity =
                e_source_get_extension(self.0.as_ptr(), E_SOURCE_EXTENSION_SECURITY.as_ptr())
                    .cast();
            e_source_security_set_secure(security, if secure { GTRUE } else { GFALSE });
        }
        self
    }

    fn has_extension(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a NUL-terminated name.
        unsafe { e_source_has_extension(self.0.as_ptr(), name.as_ptr()) != GFALSE }
    }

    fn parts_of(&self) -> Parts {
        // SAFETY: a live source.
        unsafe { parts_of(self.0.as_ptr()) }
    }

    fn server_of(&self) -> Result<Server, SourceError> {
        // SAFETY: a live source.
        unsafe { server_of(self.0.as_ptr()) }
    }

    fn user_of(&self) -> Option<String> {
        // SAFETY: a live source.
        unsafe { user_of(self.0.as_ptr()) }
    }
}

/// A valid account: TLS, an explicit port, a user and an authentication method.
fn account() -> TestSource {
    TestSource::new()
        .enabled(true)
        .authentication(
            "jmap.example.com",
            8443,
            Some("vera@example.com"),
            Some("plain/password"),
        )
        .secure(true)
}

#[test]
fn an_account_that_says_nothing_about_its_parts_has_all_of_them_and_is_not_given_any() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // `e_source_get_extension` creates the extension that it gets, as
        // `resource_id_of` also finds. A direct read of `[Collection]` would add
        // three flags to the keyfile of the account, which EDS writes back. Thus
        // the read must check for the extension first. A missing extension means
        // ALL, because `e_collection_backend_get_part_enabled()` returns TRUE.
        let source = TestSource::new().enabled(true);

        assert_eq!(source.parts_of(), Parts::ALL);
        assert!(
            !source.has_extension(E_SOURCE_EXTENSION_COLLECTION),
            "reading the parts gave the account a [Collection] extension it did not have"
        );
    });
}

#[test]
fn a_disabled_account_has_no_parts_whatever_its_extension_says() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // `e_collection_backend_get_part_enabled()` checks the `enabled` property
        // of the source before the extension. If a disabled account populated, the
        // sidebar would show children of an account that is not there.
        let source = TestSource::new().enabled(false).parts(Parts::ALL);

        assert_eq!(source.parts_of(), Parts::NONE);
        assert!(!source.parts_of().any(), "and so has nothing to populate");
    });
}

#[test]
fn each_tick_is_read_as_the_part_it_is() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // Each flag must go to its own part. EDS spells the calendar flag in the
        // singular (`calendar-enabled`) and the contacts flag in the plural. A swap
        // is easy to miss in the code.
        for parts in [
            Parts::ALL,
            Parts::NONE,
            Parts {
                mail: true,
                contacts: false,
                calendars: false,
            },
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
            let source = TestSource::new().enabled(true).parts(parts);

            assert_eq!(source.parts_of(), parts, "{parts:?} did not read back");
        }
    });
}

#[test]
fn a_switched_off_part_is_read_off_the_account_and_not_off_the_collections() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // `Fanout::discover` uses this condition before each listing. `wants` is in
        // the sync crate, but it gets only the `Parts` from `parts_of`. Thus the
        // two must agree about each flag.
        let source = TestSource::new().enabled(true).parts(Parts {
            mail: true,
            contacts: true,
            calendars: false,
        });

        assert!(source.parts_of().wants(ChildKind::AddressBook));
        assert!(!source.parts_of().wants(ChildKind::Calendar));
    });
}

#[test]
fn an_account_names_the_server_its_children_will_repeat() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // EDS binds only `oauth2-support` to a child, so the child gets none of
        // these fields from EDS. Without each field, a child cannot connect.
        let server = account().server_of().expect("a well-formed account");

        assert_eq!(
            server.connection,
            Connection {
                host: "jmap.example.com".to_owned(),
                port: Some(8443),
                user: Some("vera@example.com".to_owned()),
                auth_method: Some("plain/password".to_owned()),
                secure: true,
            }
        );
        assert_eq!(
            server.target,
            ConnectTarget::Origin("https://jmap.example.com:8443".into())
        );
    });
}

#[test]
fn the_server_this_backend_contacts_is_the_one_its_children_are_given() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // The origin and the connection come from one read. This backend gets
        // `/.well-known/jmap` itself, and each child makes its origin from the
        // copied fields. Two reads could disagree. Then the account would discover
        // collections on one server and get them from a different server.
        let server = account().server_of().expect("a well-formed account");
        let child = Child {
            resource_id: ChildKind::AddressBook.resource_id(&Id::new("AB1")),
            kind: ChildKind::AddressBook,
            display_name: "Personal".to_owned(),
            account_id: Id::new("A1"),
            collection_id: Id::new("AB1"),
            is_default: false,
            color: None,
            read_only: false,
            remote_deletable: true,
        };

        let settings = child.settings(&server.connection);
        let value = |group, key| {
            settings
                .iter()
                .find(|setting| (setting.group, setting.key) == (group, key))
                .map(|setting| setting.value.as_str())
        };

        assert_eq!(value("Authentication", "Host"), Some("jmap.example.com"));
        assert_eq!(value("Authentication", "Port"), Some("8443"));
        assert_eq!(value("Security", "Method"), Some("tls"));
        let ConnectTarget::Origin(origin) = &server.target else {
            panic!(
                "an explicit port names an endpoint, not a domain: {:?}",
                server.target
            );
        };
        assert!(
            origin.contains(value("Authentication", "Host").expect("a host")),
            "the origin {origin} was assembled from some other host than the children got",
        );
    });
}

#[test]
fn a_port_the_account_does_not_name_is_left_unnamed_rather_than_zero() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // The keyfile uses 0 for "not set", and a missing key also reads as 0. If
        // passed on, the children would get `Port=0`, and this backend would use
        // port zero, not the default port of the scheme.
        let source = TestSource::new()
            .enabled(true)
            .authentication("jmap.example.com", 0, None, None)
            .secure(true);
        let server = source.server_of().expect("a port is not required");

        assert_eq!(server.connection.port, None);
        assert_eq!(
            server.target,
            ConnectTarget::Domain("jmap.example.com".into())
        );
    });
}

#[test]
fn an_account_with_no_security_extension_is_secure_and_is_not_given_one() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // `ESourceSecurity:secure` is FALSE by default. Thus a direct read cannot
        // tell a missing [Security] group from TLS that the user disabled. Plain
        // HTTP for a missing group would downgrade each hand-written account and
        // its children. `SourceConfig` reads a missing group as TLS. This read must
        // do the same, or the account and its children use different schemes.
        let source =
            TestSource::new()
                .enabled(true)
                .authentication("jmap.example.com", 0, None, None);
        let server = source.server_of().expect("a well-formed account");

        assert!(server.connection.secure);
        assert_eq!(
            server.target,
            ConnectTarget::Domain("jmap.example.com".into())
        );
        assert!(
            !source.has_extension(E_SOURCE_EXTENSION_SECURITY),
            "reading the security setting gave the account a [Security] extension it did not have"
        );
    });
}

#[test]
fn an_account_that_names_no_server_is_refused_without_being_given_an_empty_group() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // The result is `MissingHost`, not a fan-out with no server. The read
        // also must not create the extension. EDS writes the account source back
        // to disk, so an empty `[Authentication]` group would change the file.
        let source = TestSource::new().enabled(true);

        assert_eq!(source.server_of(), Err(SourceError::MissingHost));
        assert!(
            !source.has_extension(E_SOURCE_EXTENSION_AUTHENTICATION),
            "reading the host gave the account an [Authentication] extension it did not have"
        );
    });
}

#[test]
fn a_host_that_is_not_a_bare_host_name_is_refused_before_any_child_gets_it() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // The origin is made by concatenation, so the host field is not only data.
        // Each child validates the host again. But by then, the `.source` file of
        // each child contains the string, and this backend has contacted the host.
        for host in ["evil.example.com/x", "jmap.example.com:443", "http://jmap"] {
            let source = TestSource::new()
                .enabled(true)
                .authentication(host, 0, None, None)
                .secure(true);

            assert_eq!(
                source.server_of(),
                Err(SourceError::InvalidHost(host.to_owned())),
                "{host:?} was accepted as a server"
            );
        }
    });
}

#[test]
fn a_plain_http_account_is_refused_unless_it_stays_on_the_machine() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // The book and calendar backends apply the same rule through the same
        // function. The collection backend contacts the server first, and only it
        // writes the setting into the children.
        let remote = TestSource::new()
            .enabled(true)
            .authentication("jmap.example.com", 0, None, None)
            .secure(false);

        assert_eq!(
            remote.server_of(),
            Err(SourceError::InsecureTransport(
                "jmap.example.com".to_owned()
            ))
        );

        // A loopback server, as for the mock, still works, and the children get
        // an explicit `none`. A child reads a missing `[Security]` as TLS. Without
        // `none`, it would refuse the server of a plain-HTTP account.
        let local = TestSource::new()
            .enabled(true)
            .authentication("127.0.0.1", 31415, None, None)
            .secure(false);
        let server = local.server_of().expect("loopback needs no TLS");

        assert_eq!(
            server.target,
            ConnectTarget::Origin("http://127.0.0.1:31415".into())
        );
        assert!(!server.connection.secure);
    });
}

#[test]
fn the_user_an_account_names_is_read_without_the_host_being_looked_at() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // `populate` needs only one field of `[Authentication]`: the user. It
        // decides between a password request and an anonymous authentication.
        // This must not need `server_of`. An account with a user and an invalid
        // host must reach `authenticate_sync`, which reports the host error in a
        // `GError`. A populate must not treat it as anonymous.
        let source = TestSource::new().enabled(true).authentication(
            "jmap.example.com/nonsense",
            0,
            Some("vera@example.com"),
            None,
        );

        assert_eq!(source.user_of().as_deref(), Some("vera@example.com"));
        assert!(
            source.server_of().is_err(),
            "the host in this test is meant to be one server_of refuses"
        );
    });
}

#[test]
fn an_account_that_names_no_user_reads_as_anonymous_and_is_not_given_a_group() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // `jmap_backend_core::connect::credentials` reads `None` as intentionally
        // anonymous, so a populate must not ask for a password. Also, the read
        // must not create the extension in the file of the account.
        let source = TestSource::new().enabled(true);

        assert_eq!(source.user_of(), None);
        assert!(
            !source.has_extension(E_SOURCE_EXTENSION_AUTHENTICATION),
            "reading the user gave the account an [Authentication] extension it did not have"
        );
    });
}

#[test]
fn a_user_key_that_is_present_but_empty_is_no_user_at_all() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // `User=` in a keyfile reads as "", and the EDS collection backends test
        // `user && *user` for this case. Both forms must give the same decision.
        // `credentials()` returns `CredentialsRequired` for a named user with no
        // password. Thus an empty user read as a user would cause a password
        // prompt for an anonymous account.
        let source =
            TestSource::new()
                .enabled(true)
                .authentication("jmap.example.com", 0, Some(""), None);

        assert_eq!(source.user_of(), None);
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_collection_source_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}

/// The race tests write 20,000 values; this is more time than the default.
const RACE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Sets the `[Authentication]` user of `source`.
///
/// # Safety
///
/// `source` is a live source and `user` is a NUL-terminated string.
unsafe fn set_user(source: *mut ESource, user: *const std::ffi::c_char) {
    // SAFETY: by the contract of this function.
    unsafe {
        let auth = e_source_get_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr());
        e_source_authentication_set_user(auth.cast(), user);
    }
}

/// Sets the `[Authentication]` method of `source`.
///
/// # Safety
///
/// As [`set_user`].
unsafe fn set_method(source: *mut ESource, method: *const std::ffi::c_char) {
    // SAFETY: by the contract of this function.
    unsafe {
        let auth = e_source_get_extension(source, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr());
        e_source_authentication_set_method(auth.cast(), method);
    }
}

#[test]
fn user_of_on_another_thread_copies_a_user_the_main_thread_is_replacing() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout_duration(RACE_TIMEOUT, || {
        let source = account();
        read_while_another_thread_writes(
            source.0.as_ptr(),
            long_values("@example.com"),
            set_user,
            // SAFETY: a live source.
            |source| unsafe { user_of(source) },
            &[Some("vera@example.com".into())],
        );
    });
}

#[test]
fn server_of_on_another_thread_copies_a_user_the_main_thread_is_replacing() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout_duration(RACE_TIMEOUT, || {
        let source = account();
        read_while_another_thread_writes(
            source.0.as_ptr(),
            long_values("@example.com"),
            set_user,
            // SAFETY: a live source.
            |source| {
                unsafe { server_of(source) }
                    .expect("a valid account")
                    .connection
                    .user
            },
            &[Some("vera@example.com".into())],
        );
    });
}

#[test]
fn server_of_on_another_thread_copies_a_method_the_main_thread_is_replacing() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout_duration(RACE_TIMEOUT, || {
        let source = account();
        read_while_another_thread_writes(
            source.0.as_ptr(),
            long_values("/password"),
            set_method,
            // SAFETY: a live source.
            |source| {
                unsafe { server_of(source) }
                    .expect("a valid account")
                    .connection
                    .auth_method
            },
            &[Some("plain/password".into())],
        );
    });
}
