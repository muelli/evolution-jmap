// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The account that a setup writes, on real `ESource`s. The tests read it back
// with the reader of the collection backend, not with assertions of this crate.
//
// That is the purpose of this file. The `collection_source` tests of
// `jmap-backend-collection` use sources built by hand with the EDS setters.
// They continue to pass if this crate writes the host into the wrong group.
// Assertions of this crate on its own output have the same blind spot. Neither
// tests the join: an account that the setup commits must be an account that
// the backend of the registry recognises. A gap there is not a failed
// operation. It is an account that shows in the sidebar but never makes a
// child, and this failure writes nothing in a log.

use std::ffi::CString;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_COLLECTION, E_SOURCE_EXTENSION_SECURITY,
    ESource, ESourceAuthentication, ESourceBackend, ESourceCollection,
    e_source_backend_get_backend_name, e_source_collection_get_identity, e_source_get_extension,
    e_source_has_extension, e_source_set_enabled,
};
// The getter for the token cache key that the installed EDS has. Refer to the
// module comment of `jmap_config::account`.
#[cfg(not(eds_credential_store_id))]
use eds_sys::e_source_authentication_get_credential_name as get_credential_id;
#[cfg(eds_credential_store_id)]
use eds_sys::e_source_authentication_get_credential_store_id as get_credential_id;
use glib_sys::{GFALSE, GTRUE};
use jmap_backend_collection::collection_source::{Server, parts_of, server_of, user_of};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::source::{ConnectTarget, OwnedSource, SourceError};
use jmap_collection_sync::Parts;
use jmap_collection_sync::child_source::Connection;
use jmap_config::account::{Account, BACKEND_NAME, apply, read};

/// An `ESource` in the state that a setup commits into: only a uid.
/// EDS uses `e_source_new_with_uid` with a NULL D-Bus object for a source read
/// from a keyfile. Thus the extensions operate as they do in the registry.
///
/// This code deliberately registers no extension GType. `apply` is the first
/// code in this process that uses `[Collection]`, `[Authentication]` or
/// `[Security]`. `e_source_get_extension` cannot find, and does not create, an
/// unregistered extension type.
struct TestSource(OwnedSource);

impl TestSource {
    fn new() -> Self {
        Self(OwnedSource::new("jmap-account"))
    }

    fn written(self, account: &Account) -> Self {
        // SAFETY: a live source.
        unsafe { apply(self.0.as_ptr(), account) };
        self
    }

    fn has_extension(&self, name: &std::ffi::CStr) -> bool {
        // SAFETY: a live source and a NUL-terminated name.
        unsafe { e_source_has_extension(self.0.as_ptr(), name.as_ptr()) != GFALSE }
    }

    /// The account as the collection backend reads it.
    fn server(&self) -> Result<Server, SourceError> {
        // SAFETY: a live source.
        unsafe { server_of(self.0.as_ptr()) }
    }

    fn parts(&self) -> Parts {
        // SAFETY: a live source.
        unsafe { parts_of(self.0.as_ptr()) }
    }

    fn user(&self) -> Option<String> {
        // SAFETY: a live source.
        unsafe { user_of(self.0.as_ptr()) }
    }

    /// `[Authentication] credential-name` (or `credential-store-id` on an EDS
    /// that has it, refer to the module comment of `jmap_config::account`).
    /// The reader has no accessor for it. Only the EDS credentials engine reads
    /// it, not the code after the collection backend.
    fn credential_name(&self) -> Option<String> {
        let auth: *mut ESourceAuthentication = self.authentication();
        // SAFETY: a live extension, which owns the string.
        unsafe { read_string(get_credential_id(auth)) }
    }

    fn authentication(&self) -> *mut ESourceAuthentication {
        assert!(
            self.has_extension(E_SOURCE_EXTENSION_AUTHENTICATION),
            "no [Authentication] group was written at all"
        );
        // SAFETY: the extension is present, so this returns the extension of
        // the source.
        unsafe {
            e_source_get_extension(self.0.as_ptr(), E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr())
                .cast()
        }
    }

    fn uid(&self) -> String {
        // SAFETY: a live source, which owns the uid. The uid is never NULL.
        unsafe { read_string(eds_sys::e_source_get_uid(self.0.as_ptr())) }
            .expect("a constructed ESource always has a uid")
    }

    /// The two fields of `[Collection]`, which the reader has no accessor for.
    /// The registry uses the backend name, not the backend. No code uses the
    /// identity yet.
    fn collection(&self) -> *mut ESourceCollection {
        assert!(
            self.has_extension(E_SOURCE_EXTENSION_COLLECTION),
            "no [Collection] group was written at all"
        );
        // SAFETY: the extension is present, so this returns the extension of
        // the source.
        unsafe {
            e_source_get_extension(self.0.as_ptr(), E_SOURCE_EXTENSION_COLLECTION.as_ptr()).cast()
        }
    }

    fn backend_name(&self) -> Option<String> {
        let collection: *mut ESourceBackend = self.collection().cast();
        // SAFETY: `ESourceCollection` derives from `ESourceBackend`, and the
        // extension owns the string.
        unsafe { read_string(e_source_backend_get_backend_name(collection)) }
    }

    fn identity(&self) -> Option<String> {
        // SAFETY: a live extension of the type that the name selects.
        unsafe { read_string(e_source_collection_get_identity(self.collection())) }
    }

    /// The account as the *setup* reads it. The widgets get their values from
    /// it, and `check_complete` examines it.
    fn account(&self) -> Account {
        // SAFETY: a live source.
        unsafe { read(self.0.as_ptr()) }
    }

    fn set_enabled(self, enabled: bool) -> Self {
        // SAFETY: a live source, and the flag is a plain property.
        unsafe { e_source_set_enabled(self.0.as_ptr(), if enabled { GTRUE } else { GFALSE }) };
        self
    }
}

/// An account with all fields filled in. Thus a field written into the wrong
/// group gives a wrong value that the test sees.
fn account() -> Account {
    Account {
        identity: "vera@example.com".to_owned(),
        connection: Connection {
            host: "jmap.example.com".to_owned(),
            port: Some(8443),
            user: Some("vera".to_owned()),
            auth_method: Some("plain/password".to_owned()),
            secure: true,
        },
        parts: Parts::ALL,
        oauth2_registered: false,
    }
}

#[test]
fn the_server_written_is_the_server_the_collection_backend_reads_back() {
    let source = TestSource::new().written(&account());

    let server = source.server().expect("the account names a server");
    assert_eq!(server.connection, account().connection);
}

#[test]
fn the_origin_the_backend_will_fetch_the_session_document_from_is_the_typed_one() {
    let source = TestSource::new().written(&account());

    let server = source.server().expect("the account names a server");
    assert_eq!(
        server.target,
        ConnectTarget::Origin("https://jmap.example.com:8443".into())
    );
}

#[test]
fn the_backend_name_written_is_the_one_the_registry_looks_the_factory_up_by() {
    let source = TestSource::new().written(&account());

    assert_eq!(source.backend_name().as_deref(), Some(BACKEND_NAME));
    assert_eq!(
        BACKEND_NAME.as_bytes(),
        jmap_backend_collection::factory::FACTORY_NAME.to_bytes(),
        "the account names a factory the module does not register"
    );
}

#[test]
fn the_identity_is_written_where_the_collection_extension_holds_it() {
    let source = TestSource::new().written(&account());

    assert_eq!(source.identity().as_deref(), Some("vera@example.com"));
    // The identity is not the login name, which is the other string on the page.
    assert_eq!(source.user().as_deref(), Some("vera"));
}

#[test]
fn the_parts_switched_on_are_the_parts_the_backend_fans_out_to() {
    let mut account = account();
    account.parts = Parts {
        mail: false,
        contacts: true,
        calendars: false,
    };
    let source = TestSource::new().written(&account);

    assert_eq!(source.parts(), account.parts);
}

#[test]
fn an_account_with_no_part_switched_on_is_written_as_one() {
    let mut account = account();
    account.parts = Parts::NONE;
    let source = TestSource::new().written(&account);

    // Not `Parts::ALL`, which a source with no `[Collection]` group reads as.
    // `e_collection_backend_get_part_enabled` answers TRUE for a source with no
    // value. Thus the reader cannot tell "all off" from "no value", and the
    // writer must write the value.
    assert_eq!(source.parts(), Parts::NONE);
}

#[test]
fn a_plain_text_account_is_written_as_one_rather_than_left_to_default() {
    // Loopback, because the shared `origin` rules permit plain text only to
    // that host. The mock-server recipe documents this account, and an
    // unattended run of this project makes it.
    let mut account = account();
    account.connection.secure = false;
    account.connection.host = "127.0.0.1".to_owned();
    account.connection.port = Some(8080);
    let source = TestSource::new().written(&account);

    assert!(
        source.has_extension(E_SOURCE_EXTENSION_SECURITY),
        "an account that turned TLS off has to say so: the reader takes an \
         absent [Security] group to mean TLS"
    );
    let server = source.server().expect("the account names a server");
    assert!(!server.connection.secure);
    assert_eq!(
        server.target,
        ConnectTarget::Origin("http://127.0.0.1:8080".into())
    );
}

#[test]
fn a_public_host_in_the_clear_is_written_faithfully_and_refused_by_the_reader() {
    // The writer is not the gate, and must not secretly become one. If it
    // silently changed an account to TLS, the file disagrees with what the user
    // saw. `origin`, one layer down and shared by all backends, stops plain-text
    // requests. Thus this test makes sure that the refusal stays after a write
    // to a source and a read back, not only for hand-built `Connection`s.
    //
    // `check_complete` does the UI part: it tells the user before the commit,
    // not after the first sync fails
    // (`plaintext_to_a_server_that_is_not_this_machine_is_refused` in
    // `tests/complete.rs`).
    let mut account = account();
    account.connection.secure = false;
    let source = TestSource::new().written(&account);

    assert_eq!(
        source.server(),
        Err(SourceError::InsecureTransport(
            "jmap.example.com".to_owned()
        ))
    );
}

#[test]
fn an_account_that_names_no_port_leaves_the_scheme_to_choose_one() {
    let mut account = account();
    account.connection.port = None;
    let source = TestSource::new().written(&account);

    let server = source.server().expect("the account names a server");
    // The keyfile writes "not set" as zero. It reads back as `None`, not as a
    // request for port zero.
    assert_eq!(server.connection.port, None);
    assert_eq!(
        server.target,
        ConnectTarget::Domain("jmap.example.com".into())
    );
}

#[test]
fn an_anonymous_account_is_read_back_as_anonymous() {
    let mut account = account();
    account.connection.user = None;
    account.connection.auth_method = None;
    let source = TestSource::new().written(&account);

    assert_eq!(source.user(), None);
    let server = source.server().expect("the account names a server");
    assert_eq!(server.connection.user, None);
    // *Not* `None`: `ESourceAuthentication:method` has no unset state. A new
    // extension already reads "none", and NULL and "" both set it to that
    // string, not to nothing. This was checked against the installed EDS. Thus
    // an account with no method does not come back from a source as `None`.
    // No `ESource` can hold a `Connection` with that value.
    assert_eq!(server.connection.auth_method.as_deref(), Some("none"));
}

#[test]
fn an_oauth2_account_is_given_a_credential_name_of_its_own_uid() {
    // eds#663: the token storage key was the host of the account, and the
    // credentials of an OAuth2 account did not always agree with it. The key
    // is now the uid of the account. The uid stays the same when the server
    // moves, and two accounts on the same host never share it.
    let mut account = account();
    account.connection.auth_method = Some(jmap_backend_core::oauth2::OAUTH2_METHOD.to_owned());
    let source = TestSource::new().written(&account);

    assert_eq!(
        source.credential_name().as_deref(),
        Some(source.uid().as_str())
    );
}

#[test]
fn an_account_authenticated_via_the_jmap_oauth2_service_is_given_a_credential_name() {
    // The setup UI and "Look Up Account Details" both write
    // `OAUTH2_AUTH_METHOD`: the name of the specific service ("JMAP", or
    // "OAuth2Dynamic" where EDS has its own service). They never write the
    // generic `OAUTH2_METHOD` alias ("OAuth2") that the test above uses. A real
    // account never gets to `apply` with the generic name. Thus a test of only
    // that name never examines the value that a real account has.
    let mut account = account();
    account.connection.auth_method = Some(
        jmap_backend_core::oauth2::OAUTH2_AUTH_METHOD
            .to_str()
            .expect("OAUTH2_AUTH_METHOD is a fixed ASCII string")
            .to_owned(),
    );
    let source = TestSource::new().written(&account);

    assert_eq!(
        source.credential_name().as_deref(),
        Some(source.uid().as_str())
    );
}

#[test]
fn a_password_account_is_never_given_a_credential_name() {
    // The CAUTION from the agreed design: for all methods other than OAuth2, a
    // non-empty credential-name changes the parameter name that the credentials
    // engine gives to `authenticate`. Thus a password account must never have
    // one.
    let source = TestSource::new().written(&account());

    assert_eq!(
        source.credential_name(),
        None,
        "a plain/password account was given a credential-name"
    );
}

#[test]
fn switching_an_account_away_from_oauth2_clears_its_credential_name() {
    // `apply` is idempotent in the strong sense of the module comment: after
    // it, the source holds this account and no value from before. Assume the
    // user changes an account back to a password. A stale credential-name is
    // then worse than none. It silently changes the key that the credentials
    // engine uses to find the password.
    let mut oauth2 = account();
    oauth2.connection.auth_method = Some(jmap_backend_core::oauth2::OAUTH2_METHOD.to_owned());
    let source = TestSource::new().written(&oauth2);
    assert!(
        source.credential_name().is_some(),
        "the test did not start from an OAuth2 account"
    );

    let source = source.written(&account());

    assert_eq!(
        source.credential_name(),
        None,
        "a stale credential-name survived switching the account off OAuth2"
    );
}

#[test]
fn committing_an_account_that_dropped_its_user_clears_the_one_that_was_there() {
    // An account that the user *edits*: the main case for a setup, which a
    // test on a new source cannot reach. If the writer skipped fields with no
    // value, the old login name and the old server stay. The account then
    // continues to ask libsecret for a password under a deleted name.
    let source = TestSource::new().written(&account());

    let second = Account {
        identity: "vera@example.org".to_owned(),
        connection: Connection {
            host: "jmap.example.org".to_owned(),
            port: None,
            user: None,
            auth_method: None,
            secure: true,
        },
        parts: Parts::NONE,
        oauth2_registered: false,
    };
    let source = source.written(&second);

    let server = source.server().expect("the account names a server");
    assert_eq!(
        server.connection,
        Connection {
            // The one field that cannot be cleared, because EDS has no unset
            // state for it (refer to `an_anonymous_account_is_read_back_as_anonymous`).
            // "none" is the correct answer: it is the new meaning of the
            // account, not the old "plain/password".
            auth_method: Some("none".to_owned()),
            ..second.connection.clone()
        }
    );
    assert_eq!(source.user(), None);
    assert_eq!(source.identity().as_deref(), Some("vera@example.org"));
    assert_eq!(source.parts(), Parts::NONE);
}

#[test]
fn the_three_groups_an_account_is_made_of_are_all_created() {
    let source = TestSource::new().written(&account());

    // The reader looks for each group before it reads it, and answers a
    // documented default for an absent group. Thus a missing group gives a
    // plausible answer, not a failure. The exception is `[Collection]`: without
    // it, the registry never gives the file to a collection factory.
    for name in [
        E_SOURCE_EXTENSION_COLLECTION,
        E_SOURCE_EXTENSION_AUTHENTICATION,
        E_SOURCE_EXTENSION_SECURITY,
    ] {
        assert!(
            source.has_extension(name),
            "no {} group was written",
            name.to_string_lossy()
        );
    }
}

#[test]
fn an_interior_nul_in_a_typed_string_is_truncated_rather_than_lost() {
    // A user can paste a NUL, and a C string cannot hold it. The write must not
    // be skipped: all operations fail on an account with no host.
    let mut account = account();
    account.identity = "vera@example.com\0and more".to_owned();
    account.connection.host = "jmap.example.com\0.evil.example".to_owned();
    let source = TestSource::new().written(&account);

    assert_eq!(source.identity().as_deref(), Some("vera@example.com"));
    let server = source.server().expect("the account names a server");
    assert_eq!(server.connection.host, "jmap.example.com");
}

// `read`, the inverse of `apply`. It gives the account for the widgets, and the
// account that `check_complete` and `commit_changes` get. It is correct if it
// and `apply` describe one keyfile. Thus most tests below write an account and
// read it back. The other tests compare it with the registry reader on the same
// file.

#[test]
fn an_account_that_was_written_is_the_account_that_is_read_back() {
    let source = TestSource::new().written(&account());

    assert_eq!(source.account(), account());
}

#[test]
fn an_account_with_nothing_optional_in_it_round_trips_but_for_the_method() {
    let mut written = account();
    written.connection.user = None;
    written.connection.auth_method = None;
    written.connection.port = None;
    written.parts = Parts::NONE;
    let source = TestSource::new().written(&written);

    assert_eq!(
        source.account(),
        Account {
            connection: Connection {
                // The one field that changes. `read` gives the value of the
                // source and does not map it back. `ESourceAuthentication:method`
                // has no unset state, so a written account with no method *is*
                // "none" (refer to `an_anonymous_account_is_read_back_as_anonymous`).
                // If `read` changed it to `None`, this crate and the collection
                // backend disagree about the same string.
                auth_method: Some("none".to_owned()),
                ..written.connection.clone()
            },
            ..written
        }
    );
}

#[test]
fn a_plain_text_account_is_read_back_as_one() {
    let mut written = account();
    written.connection.secure = false;
    written.connection.host = "127.0.0.1".to_owned();
    written.connection.port = Some(8080);
    let source = TestSource::new().written(&written);

    // The account from the mock-server recipe. Here, "TLS off" versus "no
    // value" decides if a password goes out in plain text. An absent
    // `[Security]` group reads as TLS, so this reads as insecure only because
    // `apply` wrote it.
    assert_eq!(source.account(), written);
}

#[test]
fn the_connection_the_setup_reads_back_is_the_one_the_registry_will_read() {
    let source = TestSource::new().written(&account());

    // The join, in the direction that `read` adds. Two descriptions of one
    // keyfile: one fills the entries of the dialog, and one opens the
    // connection. If this crate read a field from the wrong group, the user
    // sees a server that the backend never connects to.
    let server = source.server().expect("the account names a server");
    assert_eq!(source.account().connection, server.connection);
    assert_eq!(source.account().parts, source.parts());
}

#[test]
fn a_source_that_says_nothing_reads_as_an_account_with_nothing_in_it() {
    let source = TestSource::new();

    assert_eq!(
        source.account(),
        Account {
            identity: String::new(),
            connection: Connection {
                // Empty, not absent: `Account` holds the entry values, and an
                // entry with no input is the empty string. `complete::check`
                // changes it to the `None` that the shared `origin` rules use.
                host: String::new(),
                port: None,
                user: None,
                auth_method: None,
                // Absent `[Security]` is TLS. This is the rule of the
                // collection backend, not a second rule. The `secure` property
                // of an absent extension reads FALSE. Then a dialog on a
                // hand-written account offers to commit it with TLS off.
                secure: true,
            },
            // Absent `[Collection]` is all parts, because
            // `e_collection_backend_get_part_enabled` answers that for a source
            // with no value. The dialog must show the account that the registry
            // sees, not an account with fewer parts.
            parts: Parts::ALL,
            // Absent `[JMAP OAuth2]` reads as `Config::default()`, whose
            // `client_id` is `None`.
            oauth2_registered: false,
        }
    );
}

#[test]
fn reading_an_account_does_not_add_a_group_to_it() {
    let source = TestSource::new();

    let _ = source.account();

    // `read` gets the file of the user, and `e_source_get_extension` creates a
    // missing extension. If a read added three groups, to open the account
    // editor and push Cancel becomes a write.
    for name in [
        E_SOURCE_EXTENSION_COLLECTION,
        E_SOURCE_EXTENSION_AUTHENTICATION,
        E_SOURCE_EXTENSION_SECURITY,
    ] {
        assert!(
            !source.has_extension(name),
            "reading created the {} group",
            name.to_string_lossy()
        );
    }
}

#[test]
fn the_parts_of_a_disabled_account_are_still_the_ones_it_was_set_up_with() {
    let mut written = account();
    written.parts = Parts {
        mail: true,
        contacts: false,
        calendars: true,
    };
    let source = TestSource::new().written(&written).set_enabled(false);

    // Deliberately *not* the answer of the registry. This is the one place
    // where the two readers differ on purpose. `parts_of` includes the
    // `enabled` flag of the source, because a disabled account has no parts to
    // populate. `read` must not, because `enabled` is not a field of `Account`
    // and `apply` writes all three switches each time. If `read` answered
    // `NONE`, the user sees three cleared check boxes, and the commit writes
    // them. Then "hide this account for now" permanently loses its parts.
    assert_eq!(source.parts(), Parts::NONE);
    assert_eq!(source.account(), written);
}

#[test]
fn a_read_on_another_thread_copies_strings_the_main_thread_is_replacing() {
    // A config lookup worker reads the collection source on a pool thread. At
    // the same time, the widgets of the assistant can set its host on the main
    // thread.
    struct Shared(*mut ESource);
    // SAFETY: an `ESource` and its extensions guard their state with a lock.
    unsafe impl Send for Shared {}
    unsafe impl Sync for Shared {}

    let source = TestSource::new().written(&account());
    let shared = std::sync::Arc::new(Shared(source.0.as_ptr()));
    let hosts = ["a".repeat(4000) + ".example", "b".repeat(9000) + ".example"];
    let names: Vec<CString> = hosts
        .iter()
        .map(|host| CString::new(host.as_str()).expect("no NUL"))
        .collect();
    let writer = {
        let shared = std::sync::Arc::clone(&shared);
        std::thread::spawn(move || {
            // SAFETY: a live source with the `[Authentication]` extension that
            // `apply` wrote.
            let auth = unsafe {
                eds_sys::e_source_get_extension(
                    shared.0,
                    eds_sys::E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr(),
                )
            };
            for round in 0..20_000 {
                // SAFETY: a live extension and a NUL-terminated host.
                unsafe {
                    eds_sys::e_source_authentication_set_host(
                        auth.cast(),
                        names[round % 2].as_ptr(),
                    )
                };
            }
        })
    };
    while !writer.is_finished() {
        // SAFETY: a live source.
        let host = unsafe { read(shared.0) }.connection.host;
        assert!(
            hosts.contains(&host) || host == account().connection.host,
            "a torn host of {} bytes",
            host.len()
        );
    }
    writer.join().expect("writer");
}
