// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The account a setup writes, against real `ESource`s — and read back with the
// collection backend's own reader rather than with assertions of this crate's
// own devising.
//
// That is the whole point of the file. `jmap-backend-collection`'s
// `collection_source` is tested against sources built by hand with the EDS
// setters, and would go on passing if this crate wrote the host into the wrong
// group; this crate could assert that it wrote what it meant to write and be
// equally blind. What neither covers alone is the join — that an account
// committed by the setup is an account the registry's backend recognises. A gap
// there is not a failed operation: it is an account that appears in the sidebar
// and never produces a child, which is precisely the failure mode that leaves
// nothing in any log.

use std::ffi::CString;
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_COLLECTION, E_SOURCE_EXTENSION_SECURITY,
    ESource, ESourceBackend, ESourceCollection, e_source_backend_get_backend_name,
    e_source_collection_get_identity, e_source_get_extension, e_source_has_extension,
    e_source_new_with_uid, e_source_set_enabled,
};
use glib_sys::{GFALSE, GTRUE};
use gobject_sys::g_object_unref;
use jmap_backend_collection::collection_source::{Server, parts_of, server_of, user_of};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::source::SourceError;
use jmap_collection_sync::Parts;
use jmap_collection_sync::child_source::Connection;
use jmap_config::account::{Account, BACKEND_NAME, apply, read};

/// An `ESource` in the state a setup commits into: a uid and nothing else.
/// `e_source_new_with_uid` with a NULL D-Bus object is what EDS itself uses for
/// a source read from a keyfile, so the extension machinery behaves as it does
/// in the registry.
///
/// Nothing here registers an extension GType, deliberately: `apply` is the
/// first thing in this process to touch `[Collection]`, `[Authentication]` or
/// `[Security]`, and an unregistered extension type is one
/// `e_source_get_extension` cannot find and does not create.
struct TestSource(*mut ESource);

impl TestSource {
    fn new() -> Self {
        let uid = CString::new("jmap-account").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a GError
        // out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    fn written(self, account: &Account) -> Self {
        // SAFETY: a live source.
        unsafe { apply(self.0, account) };
        self
    }

    fn has_extension(&self, name: &std::ffi::CStr) -> bool {
        // SAFETY: a live source and a NUL-terminated name.
        unsafe { e_source_has_extension(self.0, name.as_ptr()) != GFALSE }
    }

    /// The account as the collection backend reads it.
    fn server(&self) -> Result<Server, SourceError> {
        // SAFETY: a live source.
        unsafe { server_of(self.0) }
    }

    fn parts(&self) -> Parts {
        // SAFETY: a live source.
        unsafe { parts_of(self.0) }
    }

    fn user(&self) -> Option<String> {
        // SAFETY: a live source.
        unsafe { user_of(self.0) }
    }

    /// `[Collection]`'s own two fields, which the reader has no accessor for:
    /// the backend name is the registry's business rather than the backend's,
    /// and the identity is nobody's yet.
    fn collection(&self) -> *mut ESourceCollection {
        assert!(
            self.has_extension(E_SOURCE_EXTENSION_COLLECTION),
            "no [Collection] group was written at all"
        );
        // SAFETY: the extension is present, so this returns the source's own.
        unsafe { e_source_get_extension(self.0, E_SOURCE_EXTENSION_COLLECTION.as_ptr()).cast() }
    }

    fn backend_name(&self) -> Option<String> {
        let collection: *mut ESourceBackend = self.collection().cast();
        // SAFETY: `ESourceCollection` derives from `ESourceBackend`; the string
        // is owned by the extension.
        unsafe { read_string(e_source_backend_get_backend_name(collection)) }
    }

    fn identity(&self) -> Option<String> {
        // SAFETY: a live extension of the type the name selects.
        unsafe { read_string(e_source_collection_get_identity(self.collection())) }
    }

    /// The account as the *setup* reads it back — what the widgets are filled
    /// from and what `check_complete` is asked about.
    fn account(&self) -> Account {
        // SAFETY: a live source.
        unsafe { read(self.0) }
    }

    fn set_enabled(self, enabled: bool) -> Self {
        // SAFETY: a live source; the flag is a plain property.
        unsafe { e_source_set_enabled(self.0, if enabled { GTRUE } else { GFALSE }) };
        self
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: this holds the only reference.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// An account with every field filled in, so that a field written into the
/// wrong group has somewhere wrong to land.
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
    assert_eq!(server.origin, "https://jmap.example.com:8443");
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
    // And it is not the login name, which is the other string on the page.
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

    // Not `Parts::ALL`, which is what a source with no `[Collection]` group at
    // all reads as: the reader cannot tell "everything off" from "nothing
    // said" — `e_collection_backend_get_part_enabled` answers TRUE for a
    // source that says nothing — so the writer has to say it.
    assert_eq!(source.parts(), Parts::NONE);
}

#[test]
fn a_plain_text_account_is_written_as_one_rather_than_left_to_default() {
    // Loopback, because that is the only host the shared `origin` rules let an
    // account reach in the clear — and it is the account the mock-server recipe
    // documents, which is the one an unattended run of this project produces.
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
    assert_eq!(server.origin, "http://127.0.0.1:8080");
}

#[test]
fn a_public_host_in_the_clear_is_written_faithfully_and_refused_by_the_reader() {
    // The writer is not the gate, and must not quietly become one: an account
    // it silently "fixed" to TLS would be an account whose file disagrees with
    // what the user was shown. What stops the request going out in the clear is
    // `origin`, one layer down and shared by every backend — so what this
    // asserts is that the refusal survives being written to a source and read
    // back, rather than only holding for hand-built `Connection`s.
    //
    // The UI half of it — telling the user before they commit, rather than
    // after the first sync fails — is `check_complete`'s, and is not written
    // yet.
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
    // Zero, which is how the keyfile spells "not set", read back as `None`
    // rather than as a request for port zero.
    assert_eq!(server.connection.port, None);
    assert_eq!(server.origin, "https://jmap.example.com");
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
    // And *not* `None`: `ESourceAuthentication:method` has no unset state. A
    // fresh extension already reads "none", and NULL and "" both set it back to
    // that string rather than to nothing — checked directly against the
    // installed EDS, not assumed. So an account with no method does not
    // round-trip through a source as `None`, and a test that expected it to
    // would be describing a `Connection` no `ESource` can hold.
    assert_eq!(server.connection.auth_method.as_deref(), Some("none"));
}

#[test]
fn committing_an_account_that_dropped_its_user_clears_the_one_that_was_there() {
    // The case a setup exists for and a fresh-source test cannot reach: an
    // account being *edited*. A writer that skipped the fields it has nothing
    // to say about would leave the old login name and the old server behind,
    // and the account would go on asking libsecret for a password under a name
    // the user deleted.
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
            // for it — see `an_anonymous_account_is_read_back_as_anonymous`.
            // "none" is nevertheless the right answer here: it is what the
            // account now means, and not the "plain/password" it used to say.
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

    // The reader tests for each group before reading it and answers a
    // documented default for an absent one, so a group this never created
    // would show up as a plausible answer rather than as a failure — except
    // for `[Collection]`, whose absence means the registry never offers the
    // file to a collection factory at all.
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
    // Nothing stops a user pasting one, and a C string cannot carry it. What
    // must not happen is the write being skipped: an account with no host is
    // one every operation fails on.
    let mut account = account();
    account.identity = "vera@example.com\0and more".to_owned();
    account.connection.host = "jmap.example.com\0.evil.example".to_owned();
    let source = TestSource::new().written(&account);

    assert_eq!(source.identity().as_deref(), Some("vera@example.com"));
    let server = source.server().expect("the account names a server");
    assert_eq!(server.connection.host, "jmap.example.com");
}

// `read`, the inverse: the account the widgets are filled from, and the one
// `check_complete` and `commit_changes` are handed. Its acceptance is that it
// and `apply` describe one keyfile — so most of what follows writes an account
// and asks for it back, and the rest holds it against the reader the registry
// will use on the same file.

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
                // The one field that does not survive the trip, and the reason
                // `read` reports what the source says rather than mapping it
                // back: `ESourceAuthentication:method` has no unset state, so
                // "none" is what an account with no method *is* once written —
                // see `an_anonymous_account_is_read_back_as_anonymous`. Turning
                // it back into `None` here would be this crate disagreeing with
                // the collection backend about the same string.
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

    // The account the mock-server recipe documents, and the one case where the
    // difference between "TLS off" and "said nothing" decides whether a
    // password goes out in the clear: an absent `[Security]` group reads as
    // TLS, so this only comes back as insecure because `apply` wrote it.
    assert_eq!(source.account(), written);
}

#[test]
fn the_connection_the_setup_reads_back_is_the_one_the_registry_will_read() {
    let source = TestSource::new().written(&account());

    // The join, in the direction `read` adds: two descriptions of one keyfile,
    // one used to fill the dialog's entries and one used to open the
    // connection the account is for. A field this crate read out of the wrong
    // group would show the user a server the backend never contacts.
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
                // Empty rather than absent: `Account` is what the entries hold,
                // and an entry the user has not filled in is the empty string.
                // `complete::check` is where that becomes the `None` the shared
                // `origin` rules take.
                host: String::new(),
                port: None,
                user: None,
                auth_method: None,
                // Absent `[Security]` is TLS, which is the collection
                // backend's rule and not a second one: reading the `secure`
                // property of an extension that is not there would answer
                // FALSE, and a dialog opened on a hand-written account would
                // offer to commit it back with TLS switched off.
                secure: true,
            },
            // And absent `[Collection]` is every part, because that is what
            // `e_collection_backend_get_part_enabled` answers for a source that
            // says nothing — the dialog has to show the account the registry
            // sees, not an emptier one.
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

    // `read` is handed the user's own file, and `e_source_get_extension`
    // creates what it cannot find. A read that left three groups behind would
    // turn opening the account editor and pressing Cancel into a write.
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

    // Deliberately *not* the registry's answer, which is the one place the two
    // readers differ on purpose. `parts_of` folds in the source's own `enabled`
    // flag because a disabled account has no parts to populate; `read` must not,
    // because `enabled` is not a field of `Account` and `apply` writes all three
    // switches every time. A `read` that answered `NONE` here would show the
    // user three cleared check boxes and then commit them, turning "hide this
    // account for now" into permanently losing which parts it offered.
    assert_eq!(source.parts(), Parts::NONE);
    assert_eq!(source.account(), written);
}
