// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests for the write of a child source, with real `ESource`s. This is the
// opposite direction of `tests/resource_id.rs`. A populate writes a child once,
// and each later start reads it.
//
// Here, the write side and the read side are tested together.
// `jmap-collection-sync` tests `Child::settings` as a list of triples, which
// describes a keyfile, not an `ESource`. The tests of `resource_id_of` and
// `SourceConfig::from_source` use sources made with the EDS setters, not the
// output of this backend. These tests check the join: the settings become the
// properties that the two readers look for. If not, EDS deletes the cache file
// of the child at the next start, or the child has no server.

use std::ffi::{CStr, CString};
use std::ptr;

use eds_sys::{
    E_SOURCE_EXTENSION_ADDRESS_BOOK, E_SOURCE_EXTENSION_CALENDAR, E_SOURCE_EXTENSION_SECURITY,
    ESource, ESourceSecurity, ESourceSelectable, e_source_get_display_name, e_source_get_extension,
    e_source_has_extension, e_source_new_with_uid, e_source_security_get_secure,
    e_source_selectable_get_color,
};
use glib_sys::GFALSE;
use gobject_sys::g_object_unref;
use jmap_backend_collection::child_source::{EXTENSIONS, UnwritableSetting, apply};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::source::{ConnectTarget, SourceConfig};
use jmap_collection_sync::child_source::{
    Connection, EXTENSION_AUTHENTICATION, EXTENSION_DATA_SOURCE,
};
use jmap_collection_sync::{Child, ChildKind, Setting};
use jmap_proto::Id;

mod common;
use common::{with_timeout, with_timeout_duration};

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// An `ESource` in the state that `e_collection_backend_new_child` gives: a
/// uid and no extensions. EDS uses `e_source_new_with_uid` with a NULL D-Bus
/// object for a source from a keyfile, so the extensions behave as in a backend.
///
/// This type registers no extension GType. `apply` must do it, because in a
/// backend, `populate` calls `apply` before other code uses `[Resource]` or
/// `[Security]`.
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

    /// Writes a new child as `populate` does: all settings of the child, in
    /// their order.
    fn written(self, child: &Child, connection: &Connection) -> Self {
        // SAFETY: a live source.
        unsafe { apply(self.0, &child.settings(connection)) }.expect("a child this backend wrote");
        self
    }

    fn display_name(&self) -> Option<String> {
        // SAFETY: a live source. The source owns the string that it returns.
        unsafe { read_string(e_source_get_display_name(self.0)) }
    }

    fn has_extension(&self, name: &CStr) -> bool {
        // SAFETY: a live source and a NUL-terminated name.
        unsafe { e_source_has_extension(self.0, name.as_ptr()) != GFALSE }
    }

    /// `ESourceSecurity:secure` as the JMAP backends read it, not the written
    /// string. The two agree only if EDS spells the secure method as
    /// [`Child::settings`] does.
    fn secure(&self) -> bool {
        assert!(
            self.has_extension(E_SOURCE_EXTENSION_SECURITY),
            "no [Security] group was written at all"
        );
        // SAFETY: the extension is present, so this returns the extension that
        // the source owns.
        unsafe {
            let security: *mut ESourceSecurity =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_SECURITY.as_ptr()).cast();
            e_source_security_get_secure(security) != GFALSE
        }
    }

    /// `ESourceSelectable:color` from the "Calendar" extension. `ESourceCalendar`
    /// derives from `ESourceSelectable`, so one object has both.
    fn color(&self) -> Option<String> {
        // SAFETY: a live source. The source creates the extension when
        // necessary and owns it. The returned string is borrowed from it.
        unsafe {
            let selectable: *mut ESourceSelectable =
                e_source_get_extension(self.0, E_SOURCE_EXTENSION_CALENDAR.as_ptr()).cast();
            read_string(e_source_selectable_get_color(selectable))
        }
    }

    /// The child, read as the address book and calendar backends read their
    /// source.
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
        // SAFETY: this is the only reference.
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
        color: None,
        read_only: false,
        remote_deletable: true,
    }
}

#[test]
fn the_extension_names_are_the_ones_eds_defines() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // `jmap-collection-sync` builds without the EDS headers, so it has its
        // own keyfile group names. Only this crate sees both spellings. A
        // mismatch compiles, because `e_source_get_extension` takes a string,
        // and the setting is then not written.
        for (defined, group) in EXTENSIONS {
            assert_eq!(
                defined.to_str().expect("EDS names its groups in ASCII"),
                group
            );
        }
    });
}

#[test]
fn a_written_child_is_read_back_as_the_child_it_was_written_from() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // EDS does this round trip at each later start.
        // `collection_backend_load_resources()` reads the cached `.source` file
        // and calls `dup_resource_id`. A different string causes a second source
        // for one collection. No answer deletes the file.
        for (kind, collection) in [
            (ChildKind::AddressBook, "AB1"),
            (ChildKind::Calendar, "Cal1"),
            // The same JMAP id for both kinds. Ids are unique for each data type,
            // not for each account (RFC 8620 §1.2), so a server can send this pair.
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
    });
}

#[test]
fn a_written_child_reaches_the_server_its_account_named() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // From the connection of the account, EDS binds only `oauth2-support` to
        // the child. The address book and calendar backends get the child. Thus
        // the copied settings are all the data that these backends have.
        // `tests/child_added.rs` tests the later updates from the account. This
        // test checks the initial copy.
        let child = child(ChildKind::AddressBook, "AB1", "Personal");
        let source = TestSource::new().written(&child, &connection());

        assert_eq!(
            source.config(),
            SourceConfig {
                target: ConnectTarget::Origin("https://jmap.example.com:8443".to_owned()),
                user: Some("vera@example.com".to_owned()),
                resource_id: Some("AB1".to_owned()),
                rebase_urls: false,
            }
        );
        assert_eq!(
            source.display_name(),
            Some("Personal".to_owned()),
            "and Evolution shows the collection's name rather than a blank row"
        );
    });
}

#[test]
fn every_setting_a_child_can_be_described_by_is_one_this_writes() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // `Child::settings` is in a different crate, which can add a setting. If
        // `apply` cannot write it, the backend only logs the error. Thus this
        // test asserts that `apply` writes all settings of each child and account
        // shape.
        for kind in [ChildKind::AddressBook, ChildKind::Calendar] {
            for connection in [
                connection(),
                // No optional values, as for an account with no port and no
                // credentials.
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
    });
}

#[test]
fn a_setting_this_backend_cannot_write_is_refused_rather_than_dropped() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // `apply` must not silently skip an unknown setting. Otherwise the child
        // looks correct but is without a necessary property. The error names the
        // group and key, because the log is the only place where a user sees it.
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
                // A port is a number in `ESourceAuthentication` and a string in
                // the keyfile. This module does the conversion, so it reports the
                // failure.
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
    });
}

#[test]
fn a_child_carries_the_extension_of_its_own_kind_and_not_the_other() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // The extension sets the kind. `collection_backend_child_is_contacts()`,
        // `collection_backend_child_is_calendar()` and the kind part of the
        // resource id all use `e_source_has_extension`. A child with both would
        // go to the first factory that checks. `e_source_get_extension` creates
        // the extension that it gets, so a wrong call can cause this.
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
    });
}

#[test]
fn a_plain_http_account_writes_children_that_do_not_insist_on_tls() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // `Child::settings` writes `[Security] Method` as the keyfile string
        // "tls" or "none". The backends read `ESourceSecurity:secure`, a boolean.
        // They agree only if EDS spells the secure method the same way. If not, a
        // child of a plain-HTTP account reports a TLS error. Or, worse, the
        // children of a TLS account silently use plain HTTP.
        let child = child(ChildKind::AddressBook, "AB1", "Personal");
        let mut plain = connection();
        plain.host = "127.0.0.1".to_owned();
        plain.secure = false;

        let source = TestSource::new().written(&child, &plain);
        assert!(!source.secure());
        assert_eq!(
            source.config().target,
            ConnectTarget::Origin("http://127.0.0.1:8443".into())
        );

        let secure = TestSource::new().written(&child, &connection());
        assert!(secure.secure(), "and a TLS account's children still say so");
    });
}

#[test]
fn a_port_the_account_did_not_name_leaves_the_child_at_the_scheme_default() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // `Child::settings` omits the setting and does not write 0. An omitted
        // setting must leave the property "not set". A child with port 0 cannot
        // connect.
        let mut unported = connection();
        unported.port = None;

        let source =
            TestSource::new().written(&child(ChildKind::AddressBook, "AB1", "Personal"), &unported);

        assert_eq!(
            source.config().target,
            ConnectTarget::Domain("jmap.example.com".into())
        );
    });
}

#[test]
fn a_collection_name_with_an_interior_nul_still_produces_a_usable_child() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // The display name is the only setting with a value from the server. A
        // JSON string can contain an escaped NUL, but a C string cannot. A refused
        // write would remove the child. Thus `apply` truncates the name at the
        // NUL, as each C caller would read it. The other properties do not change.
        let named = child(ChildKind::AddressBook, "AB1", "Person\0al");
        let source = TestSource::new().written(&named, &connection());

        assert_eq!(source.display_name(), Some("Person".to_owned()));
        assert_eq!(source.resource_id(), Some("addressbook:AB1".to_owned()));
    });
}

#[test]
fn a_calendars_color_reaches_the_child_and_reads_back_as_the_one_written() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // A real backend needs this round trip. `Child::settings` describes the
        // color as a `("Calendar", "Color", …)` triple. This test checks that the
        // triple sets the `ESourceSelectable:color` property.
        let mut colored = child(ChildKind::Calendar, "Cal1", "Work");
        colored.color = Some("#ff8800".to_owned());
        let source = TestSource::new().written(&colored, &connection());

        assert_eq!(source.color(), Some("#ff8800".to_owned()));
    });
}

#[test]
fn a_calendar_the_server_named_no_color_for_gets_no_color_setting() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    with_timeout(|| {
        // Unlike the other properties of a new extension, `ESourceSelectable:color`
        // is not NULL by default. The EDS GParamSpec gives it a default ("#62a0ea",
        // the GNOME accent blue). A calendar without a server color gets that
        // default. `Child::settings` omits the setting (see the module comment on
        // omitted and empty values). Thus this backend does not replace the
        // default with an empty string, as `e_source_selectable_set_color(sel, "")`
        // would.
        let uncolored = child(ChildKind::Calendar, "Cal1", "Work");
        let source = TestSource::new().written(&uncolored, &connection());

        assert!(
            !uncolored
                .settings(&connection())
                .iter()
                .any(|setting| setting.key == "Color"),
            "an unset color must not be a Setting at all"
        );
        assert_eq!(
            source.color(),
            Some("#62a0ea".to_owned()),
            "EDS's own compiled-in default for a selectable source with no color \
             ever written to it"
        );
    });
}

#[test]
#[should_panic(expected = "test timed out after")]
fn a_blocked_child_source_test_times_out_and_fails_fast() {
    with_timeout_duration(std::time::Duration::from_millis(50), || {
        std::thread::park();
    });
}
