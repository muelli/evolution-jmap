// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Reading the *collection* source — the account itself — against real
// `ESource`s built with the EDS setters, the way `tests/resource_id.rs` does
// for a child.
//
// This is everything `populate` knows before it contacts anything: which parts
// the user left switched on, where the server is, and what each child has to
// repeat in order to reach the same one. Three of the failures here are silent
// rather than loud — a part read as on that is off fetches data the user said
// they did not want, a `[Security]` read wrong downgrades or upgrades every
// child of the account at once, and a host that is not a host is a URL aimed
// somewhere else.

use std::ffi::{CStr, CString};
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_AUTHENTICATION, E_SOURCE_EXTENSION_COLLECTION, E_SOURCE_EXTENSION_SECURITY,
    ESource, ESourceAuthentication, ESourceCollection, ESourceSecurity,
    e_source_authentication_get_type, e_source_authentication_set_host,
    e_source_authentication_set_method, e_source_authentication_set_port,
    e_source_authentication_set_user, e_source_collection_get_type,
    e_source_collection_set_calendar_enabled, e_source_collection_set_contacts_enabled,
    e_source_collection_set_mail_enabled, e_source_get_extension, e_source_has_extension,
    e_source_new_with_uid, e_source_security_get_type, e_source_security_set_secure,
    e_source_set_enabled,
};
use glib_sys::{GFALSE, GTRUE};
use gobject_sys::g_object_unref;
use jmap_backend_collection::collection_source::{Server, parts_of, server_of, user_of};
use jmap_backend_core::source::SourceError;
use jmap_collection_sync::child_source::Connection;
use jmap_collection_sync::{Child, ChildKind, Parts};
use jmap_proto::Id;

/// An `ESource` that is not backed by the registry, as in `tests/resource_id.rs`
/// — `e_source_new_with_uid` with a NULL D-Bus object is what EDS itself uses
/// for a source read from a keyfile.
struct TestSource(*mut ESource);

impl TestSource {
    fn new() -> Self {
        // `e_source_get_extension` walks the registered children of
        // `E_TYPE_SOURCE_EXTENSION`, so a type nothing has referenced yet is
        // one it cannot find. Under EDS the libraries are loaded long before
        // any source exists; in a test binary, touching the accessors is what
        // stands in for that.
        // SAFETY: no arguments, and the type system initialises itself.
        unsafe {
            e_source_collection_get_type();
            e_source_authentication_get_type();
            e_source_security_get_type();
        }

        let uid = CString::new("jmap-collection").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a GError
        // out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    /// The collection source's own `enabled` — the first thing
    /// `e_collection_backend_get_part_enabled()` looks at.
    fn enabled(self, enabled: bool) -> Self {
        // SAFETY: a live source.
        unsafe { e_source_set_enabled(self.0, if enabled { GTRUE } else { GFALSE }) };
        self
    }

    /// The `[Collection]` extension's three flags, i.e. the account's ticks.
    fn parts(self, parts: Parts) -> Self {
        // SAFETY: a live source and a header constant; the extension is
        // created on demand and owned by the source.
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
        // SAFETY: as above; every setter copies the string it is given.
        unsafe {
            let auth: *mut ESourceAuthentication =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_AUTHENTICATION.as_ptr()).cast();
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
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_SECURITY.as_ptr()).cast();
            e_source_security_set_secure(security, if secure { GTRUE } else { GFALSE });
        }
        self
    }

    fn has_extension(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a NUL-terminated name.
        unsafe { e_source_has_extension(self.0, name.as_ptr()) != GFALSE }
    }

    fn parts_of(&self) -> Parts {
        // SAFETY: a live source.
        unsafe { parts_of(self.0) }
    }

    fn server_of(&self) -> Result<Server, SourceError> {
        // SAFETY: a live source.
        unsafe { server_of(self.0) }
    }

    fn user_of(&self) -> Option<String> {
        // SAFETY: a live source.
        unsafe { user_of(self.0) }
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: the reference `e_source_new_with_uid` returned is given back
        // exactly once.
        unsafe { g_object_unref(self.0.cast()) };
    }
}

/// A well-formed account: TLS, a named port, a user and an auth method.
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
    // Two things, and the second is the trap `resource_id_of` meets too:
    // `e_source_get_extension` *creates* the extension it is asked for, so a
    // read that reached straight for `[Collection]` would write three flags
    // into the account's own keyfile — the file EDS writes back — where the
    // user had none. The absence has to be tested for, and it means ALL:
    // `e_collection_backend_get_part_enabled()` answers TRUE for a source with
    // no such extension.
    let source = TestSource::new().enabled(true);

    assert_eq!(source.parts_of(), Parts::ALL);
    assert!(
        !source.has_extension(E_SOURCE_EXTENSION_COLLECTION),
        "reading the parts gave the account a [Collection] extension it did not have"
    );
}

#[test]
fn a_disabled_account_has_no_parts_whatever_its_extension_says() {
    // The first thing `e_collection_backend_get_part_enabled()` checks is the
    // source's own `enabled`, before it looks at the extension at all. An
    // account the user switched off that still populated would put children in
    // the sidebar for an account that is not there.
    let source = TestSource::new().enabled(false).parts(Parts::ALL);

    assert_eq!(source.parts_of(), Parts::NONE);
    assert!(!source.parts_of().any(), "and so has nothing to populate");
}

#[test]
fn each_tick_is_read_as_the_part_it_is() {
    // One flag per part, and no cross-wiring: EDS spells the calendar one
    // singular (`calendar-enabled`) and the contacts one plural, so a swap here
    // is exactly the kind of thing that reads correctly and behaves backwards.
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
}

#[test]
fn a_switched_off_part_is_read_off_the_account_and_not_off_the_collections() {
    // The gate `Fanout::discover` puts in front of each listing. `wants` is the
    // sync crate's, but it is only ever asked about the `Parts` this function
    // produced, so the two have to agree about which flag is which.
    let source = TestSource::new().enabled(true).parts(Parts {
        mail: true,
        contacts: true,
        calendars: false,
    });

    assert!(source.parts_of().wants(ChildKind::AddressBook));
    assert!(!source.parts_of().wants(ChildKind::Calendar));
}

#[test]
fn an_account_names_the_server_its_children_will_repeat() {
    // A child inherits none of this — EDS binds `oauth2-support` and nothing
    // else — so every field here is one a child without it cannot connect with.
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
    assert_eq!(server.origin, "https://jmap.example.com:8443");
}

#[test]
fn the_server_this_backend_contacts_is_the_one_its_children_are_given() {
    // The reason the origin and the connection come out of one read: this
    // backend fetches `/.well-known/jmap` itself, and each child assembles its
    // own origin at the far end from the fields copied here. Two reads of one
    // source are two chances to disagree — and a disagreement is an account
    // that discovers collections from one server and fetches them from another.
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
    assert!(
        server
            .origin
            .contains(value("Authentication", "Host").expect("a host")),
        "the origin {} was assembled from some other host than the children got",
        server.origin
    );
}

#[test]
fn a_port_the_account_does_not_name_is_left_unnamed_rather_than_zero() {
    // The keyfile writes 0 for "not set", which is what an unwritten key reads
    // back as. Passing it on would give the children `Port=0` and this backend
    // an origin that asks for port zero, instead of the scheme's default.
    let source = TestSource::new()
        .enabled(true)
        .authentication("jmap.example.com", 0, None, None)
        .secure(true);
    let server = source.server_of().expect("a port is not required");

    assert_eq!(server.connection.port, None);
    assert_eq!(server.origin, "https://jmap.example.com");
}

#[test]
fn an_account_with_no_security_extension_is_secure_and_is_not_given_one() {
    // `ESourceSecurity:secure` defaults to FALSE, so an unconditional read
    // cannot tell "the keyfile has no [Security] group" from "the user turned
    // TLS off" — and answering the first with plain HTTP would downgrade every
    // hand-written account, and every child of it, at once. `SourceConfig`
    // reads the absence as TLS; this has to read it the same way or the account
    // and its children would disagree about the scheme.
    let source = TestSource::new()
        .enabled(true)
        .authentication("jmap.example.com", 0, None, None);
    let server = source.server_of().expect("a well-formed account");

    assert!(server.connection.secure);
    assert_eq!(server.origin, "https://jmap.example.com");
    assert!(
        !source.has_extension(E_SOURCE_EXTENSION_SECURITY),
        "reading the security setting gave the account a [Security] extension it did not have"
    );
}

#[test]
fn an_account_that_names_no_server_is_refused_without_being_given_an_empty_group() {
    // `MissingHost` rather than a fan-out against nothing. And the same
    // create-on-read trap once more: this is the account source, which EDS
    // writes back to disk, so a read that adds an empty `[Authentication]`
    // group edits the user's account file.
    let source = TestSource::new().enabled(true);

    assert_eq!(source.server_of(), Err(SourceError::MissingHost));
    assert!(
        !source.has_extension(E_SOURCE_EXTENSION_AUTHENTICATION),
        "reading the host gave the account an [Authentication] extension it did not have"
    );
}

#[test]
fn a_host_that_is_not_a_bare_host_name_is_refused_before_any_child_gets_it() {
    // The origin is assembled by concatenation, so the host field is not just
    // data. Each child re-validates what it was handed, but by then the same
    // string has been written into three `.source` files — and this backend has
    // already contacted whatever it named.
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
}

#[test]
fn a_plain_http_account_is_refused_unless_it_stays_on_the_machine() {
    // The same rule the book and calendar backends apply, reached through the
    // same function — this is the one place it could be forgotten, since the
    // collection backend is the first thing to contact the server and the only
    // one that writes the setting into the children.
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

    // And the mock server's shape still works, with the children told so
    // explicitly: an absent `[Security]` reads as TLS at the far end, so a
    // child of a plain-HTTP account that did not say `none` would refuse to
    // talk to the account's own server.
    let local = TestSource::new()
        .enabled(true)
        .authentication("127.0.0.1", 31415, None, None)
        .secure(false);
    let server = local.server_of().expect("loopback needs no TLS");

    assert_eq!(server.origin, "http://127.0.0.1:31415");
    assert!(!server.connection.secure);
}

#[test]
fn the_user_an_account_names_is_read_without_the_host_being_looked_at() {
    // `populate` needs one field of `[Authentication]` and only one: whether
    // the account names a user, which is what decides whether it asks EDS for a
    // password or for a straight anonymous authenticate. It must not need
    // `server_of` for that — an account with a user and a broken host has to
    // reach `authenticate_sync`, which is the vfunc that has a `GError` to
    // report the broken host through, rather than be quietly treated as
    // anonymous by a populate that could not read it.
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
}

#[test]
fn an_account_that_names_no_user_reads_as_anonymous_and_is_not_given_a_group() {
    // `None` is what `jmap_backend_core::connect::credentials` reads as
    // "anonymous on purpose", so it is also what a populate must not ask for a
    // password for. And the create-on-read trap once more, on the account's own
    // file.
    let source = TestSource::new().enabled(true);

    assert_eq!(source.user_of(), None);
    assert!(
        !source.has_extension(E_SOURCE_EXTENSION_AUTHENTICATION),
        "reading the user gave the account an [Authentication] extension it did not have"
    );
}

#[test]
fn a_user_key_that_is_present_but_empty_is_no_user_at_all() {
    // `User=` in a keyfile reads back as "", and EDS's own collection backends
    // test `user && *user` for exactly this. It matters here because the two
    // spellings must not decide differently: `credentials()` answers
    // `CredentialsRequired` for a named user with no password, so an empty user
    // read as a user is a password prompt for an account that authenticates
    // anonymously — and whatever was typed into it would then be dropped.
    let source =
        TestSource::new()
            .enabled(true)
            .authentication("jmap.example.com", 0, Some(""), None);

    assert_eq!(source.user_of(), None);
}
