// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Writing a child source, against real `ESource`s — the other direction from
// `tests/resource_id.rs`, and the direction that has to go first: a child is
// written once, at populate, and read back on every start after that.
//
// The two ends are held against each other here rather than described twice.
// `Child::settings` is tested in `jmap-collection-sync` as a list of triples,
// which is a description of a keyfile and proves nothing about an `ESource`;
// `resource_id_of` and `SourceConfig::from_source` are tested against sources
// built by hand with the EDS setters, which proves nothing about what this
// backend writes. What is not covered by either is the join: that the settings
// this backend is handed turn into the properties those two readers look for.
// A gap there is not a failed operation — it is a child whose cache file EDS
// deletes on the next start, or one whose every request goes to no server.

use std::ffi::{CStr, CString};
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_ADDRESS_BOOK, E_SOURCE_EXTENSION_CALENDAR, E_SOURCE_EXTENSION_SECURITY,
    ESource, ESourceSecurity, e_source_get_display_name, e_source_get_extension,
    e_source_has_extension, e_source_new_with_uid, e_source_security_get_secure,
};
use glib_sys::GFALSE;
use gobject_sys::g_object_unref;
use jmap_backend_collection::child_source::{EXTENSIONS, UnwritableSetting, apply};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::source::SourceConfig;
use jmap_collection_sync::child_source::{
    Connection, EXTENSION_AUTHENTICATION, EXTENSION_DATA_SOURCE,
};
use jmap_collection_sync::{Child, ChildKind, Setting};
use jmap_proto::Id;

/// An `ESource` in the state `e_collection_backend_new_child` hands one back
/// in: a uid, a parent, and nothing else. `e_source_new_with_uid` with a NULL
/// D-Bus object is what EDS itself uses for a source read from a keyfile, so
/// the extension machinery behaves as it does in a backend.
///
/// Nothing here registers an extension GType: `apply` is what has to, since a
/// backend's own `populate` reaches it before anything else has touched
/// `[Resource]` or `[Security]` on this process's behalf.
struct TestSource(*mut ESource);

impl TestSource {
    fn new() -> Self {
        let uid = CString::new("jmap-collection-child").expect("no NUL in a literal");
        let mut error = ptr::null_mut();
        // SAFETY: a NUL-terminated uid, the default main context and a GError
        // out-parameter are the documented arguments.
        let source = unsafe { e_source_new_with_uid(uid.as_ptr(), ptr::null_mut(), &mut error) };
        assert!(!source.is_null(), "e_source_new_with_uid failed");
        Self(source)
    }

    /// What `populate` does to a fresh child: everything the child describes,
    /// in the order it describes it.
    fn written(self, child: &Child, connection: &Connection) -> Self {
        // SAFETY: a live source.
        unsafe { apply(self.0, &child.settings(connection)) }.expect("a child this backend wrote");
        self
    }

    fn display_name(&self) -> Option<String> {
        // SAFETY: a live source; the string it returns is owned by it.
        unsafe { read_string(e_source_get_display_name(self.0)) }
    }

    fn has_extension(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a NUL-terminated name.
        unsafe { e_source_has_extension(self.0, name.as_ptr()) != GFALSE }
    }

    /// `ESourceSecurity:secure` as the JMAP backends read it — not the string
    /// that was written. The two are the same question only if EDS spells the
    /// secure method the way [`Child::settings`] does.
    fn secure(&self) -> bool {
        assert!(
            self.has_extension(E_SOURCE_EXTENSION_SECURITY),
            "no [Security] group was written at all"
        );
        // SAFETY: the extension is present, so this returns the source's own.
        unsafe {
            let security: *mut ESourceSecurity =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_SECURITY.as_ptr()).cast();
            e_source_security_get_secure(security) != GFALSE
        }
    }

    /// The child read the way the address book and calendar backends read the
    /// source they are handed.
    fn config(&self) -> SourceConfig {
        // SAFETY: a live source.
        unsafe { SourceConfig::from_source(self.0) }.expect("a child names its account's server")
    }

    fn resource_id(&self) -> Option<String> {
        // SAFETY: a live source.
        unsafe { jmap_backend_collection::resource_id::resource_id_of(self.0) }
    }
}

impl Drop for TestSource {
    fn drop(&mut self) {
        // SAFETY: this holds the only reference.
        unsafe { g_object_unref(self.0.cast()) };
    }
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

fn child(kind: ChildKind, collection: &str, name: &str) -> Child {
    Child {
        resource_id: kind.resource_id(&Id::new(collection)),
        kind,
        display_name: name.to_owned(),
        account_id: Id::new("A1"),
        collection_id: Id::new(collection),
        is_default: false,
        read_only: false,
    }
}

#[test]
fn the_extension_names_are_the_ones_eds_defines() {
    // `jmap-collection-sync` builds without the EDS headers, so it spells the
    // keyfile groups itself; this crate is the only one that sees both
    // spellings. A pair that drifts apart compiles — `e_source_get_extension`
    // takes a string — and produces a setting that is never written.
    for (defined, group) in EXTENSIONS {
        assert_eq!(
            defined.to_str().expect("EDS names its groups in ASCII"),
            group
        );
    }
}

#[test]
fn a_written_child_is_read_back_as_the_child_it_was_written_from() {
    // The round trip EDS performs on every start after the one that created
    // the child: `collection_backend_load_resources()` reads the cached
    // `.source` file and asks `dup_resource_id` what it is. An answer that is
    // not the string the child was created under is a second source for a
    // collection that already has one; no answer at all deletes the file.
    for (kind, collection) in [
        (ChildKind::AddressBook, "AB1"),
        (ChildKind::Calendar, "Cal1"),
        // The same JMAP id under both kinds: ids are unique per data type, not
        // per account (RFC 8620 §1.2), so this pair is what a server may
        // actually present.
        (ChildKind::AddressBook, "X1"),
        (ChildKind::Calendar, "X1"),
    ] {
        let child = child(kind, collection, "Personal");
        let source = TestSource::new().written(&child, &connection());

        assert_eq!(
            source.resource_id(),
            Some(child.resource_id.clone()),
            "a {kind:?} of {collection} did not read back as itself"
        );
    }
}

#[test]
fn a_written_child_reaches_the_server_its_account_named() {
    // A child inherits none of the account's connection from EDS, which binds
    // `oauth2-support` and nothing else — and it is the *child* the address
    // book and calendar backends are handed. So the settings copied here are
    // the whole of what those backends have to work from, and this is them
    // reading it. What follows the account *afterwards* is this backend's own
    // doing and is `tests/child_added.rs`'s subject; the copy below is what a
    // child starts life with.
    let child = child(ChildKind::AddressBook, "AB1", "Personal");
    let source = TestSource::new().written(&child, &connection());

    assert_eq!(
        source.config(),
        SourceConfig {
            origin: "https://jmap.example.com:8443".to_owned(),
            user: Some("vera@example.com".to_owned()),
            resource_id: Some("AB1".to_owned()),
        }
    );
    assert_eq!(
        source.display_name(),
        Some("Personal".to_owned()),
        "and Evolution shows the collection's name rather than a blank row"
    );
}

#[test]
fn every_setting_a_child_can_be_described_by_is_one_this_writes() {
    // `Child::settings` is the other crate's, and it can grow a setting. One
    // it grows that this does not write would not fail loudly — `apply` would
    // return the error nobody reads yet — so the exhaustiveness is asserted
    // here, over every shape a child and an account come in.
    for kind in [ChildKind::AddressBook, ChildKind::Calendar] {
        for connection in [
            connection(),
            // Everything optional left out, which is what a bare account with
            // no port and no credentials configured produces.
            Connection {
                host: "127.0.0.1".to_owned(),
                port: None,
                user: None,
                auth_method: None,
                secure: false,
            },
        ] {
            let child = child(kind, "X1", "Personal");
            let source = TestSource::new();

            // SAFETY: a live source.
            let written = unsafe { apply(source.0, &child.settings(&connection)) };

            assert_eq!(
                written,
                Ok(()),
                "a {kind:?} child of {} names a setting this backend cannot write",
                connection.host
            );
        }
    }
}

#[test]
fn a_setting_this_backend_cannot_write_is_refused_rather_than_dropped() {
    // Silently skipping an unknown setting is the failure mode this guards
    // against: the child would be created, look right, and be missing the one
    // property that makes it work. Which of the two is missing is in the
    // error, because a caller that logs it is the only way it is ever seen.
    let source = TestSource::new();

    for (setting, expected) in [
        (
            Setting {
                group: "Mail Account",
                key: "BackendName",
                value: "jmap".to_owned(),
            },
            UnwritableSetting::UnknownProperty {
                group: "Mail Account",
                key: "BackendName",
            },
        ),
        (
            Setting {
                group: EXTENSION_DATA_SOURCE,
                key: "Parent",
                value: "an-account-uid".to_owned(),
            },
            UnwritableSetting::UnknownProperty {
                group: EXTENSION_DATA_SOURCE,
                key: "Parent",
            },
        ),
        (
            // A port is a number to `ESourceAuthentication` and a string in
            // the keyfile; the conversion is this module's, so its failure is
            // too.
            Setting {
                group: EXTENSION_AUTHENTICATION,
                key: "Port",
                value: "https".to_owned(),
            },
            UnwritableSetting::WrongType {
                group: EXTENSION_AUTHENTICATION,
                key: "Port",
                value: "https".to_owned(),
            },
        ),
    ] {
        // SAFETY: a live source.
        assert_eq!(unsafe { apply(source.0, &[setting]) }, Err(expected));
    }
}

#[test]
fn a_child_carries_the_extension_of_its_own_kind_and_not_the_other() {
    // The extension *is* the kind: `collection_backend_child_is_contacts()`
    // and `…_is_calendar()` are `e_source_has_extension` calls, and so is the
    // kind half of the resource id. A child carrying both would be handed to
    // whichever factory tested first — and `e_source_get_extension` creates
    // the extension it is asked for, so writing one by mistake is a real way
    // to get there.
    for (kind, own, other) in [
        (
            ChildKind::AddressBook,
            E_SOURCE_EXTENSION_ADDRESS_BOOK,
            E_SOURCE_EXTENSION_CALENDAR,
        ),
        (
            ChildKind::Calendar,
            E_SOURCE_EXTENSION_CALENDAR,
            E_SOURCE_EXTENSION_ADDRESS_BOOK,
        ),
    ] {
        let source = TestSource::new().written(&child(kind, "X1", "Personal"), &connection());

        assert!(
            source.has_extension(own),
            "a {kind:?} child was not written as one"
        );
        assert!(
            !source.has_extension(other),
            "a {kind:?} child also claims to be the other kind"
        );
    }
}

#[test]
fn a_plain_http_account_writes_children_that_do_not_insist_on_tls() {
    // `Child::settings` writes `[Security] Method` as the keyfile string
    // "tls" or "none"; the backends read `ESourceSecurity:secure`, a boolean.
    // Those are only the same question if EDS's secure method is spelled the
    // way this writes it — untested, that is a child of a plain-HTTP account
    // that reports a TLS error its account's settings do not explain, or,
    // worse, a TLS account whose children quietly talk plain HTTP.
    let child = child(ChildKind::AddressBook, "AB1", "Personal");
    let mut plain = connection();
    plain.host = "127.0.0.1".to_owned();
    plain.secure = false;

    let source = TestSource::new().written(&child, &plain);
    assert!(!source.secure());
    assert_eq!(source.config().origin, "http://127.0.0.1:8443");

    let secure = TestSource::new().written(&child, &connection());
    assert!(secure.secure(), "and a TLS account's children still say so");
}

#[test]
fn a_port_the_account_did_not_name_leaves_the_child_at_the_scheme_default() {
    // `Child::settings` omits the setting rather than writing 0, and an
    // omitted setting has to leave the property at the value that means "not
    // set" — a child asking for port 0 connects to nothing at all.
    let mut unported = connection();
    unported.port = None;

    let source =
        TestSource::new().written(&child(ChildKind::AddressBook, "AB1", "Personal"), &unported);

    assert_eq!(source.config().origin, "https://jmap.example.com");
}

#[test]
fn a_collection_name_with_an_interior_nul_still_produces_a_usable_child() {
    // The display name is the one setting whose value is server data: a JSON
    // string may carry an escaped NUL, and a C string may not. Refusing the
    // write would be refusing the child — so it is truncated at the NUL,
    // which is what the name would have meant to every C caller downstream
    // anyway, and the properties that make the child work are unaffected.
    let named = child(ChildKind::AddressBook, "AB1", "Person\0al");
    let source = TestSource::new().written(&named, &connection());

    assert_eq!(source.display_name(), Some("Person".to_owned()));
    assert_eq!(source.resource_id(), Some("addressbook:AB1".to_owned()));
}
