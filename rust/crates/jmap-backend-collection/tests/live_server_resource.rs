// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests for the layer of `create_resource_sync` and `delete_resource_sync`
// against a real JMAP server.
//
// `jmap-collection-sync/tests/live_server.rs` tests the round trip of
// `create_collection` and `delete_collection` through real Stalwart.
// `tests/live_server.rs` in this crate tests `fan_out` on that base. Neither
// file tests the layer of the *other* two write vfuncs:
// `create_on_server`/`adopt_created` and `delete_on_server` in
// `create_resource.rs`/`delete_resource.rs`. The "New Address Book", "New
// Calendar" and "Delete" menu items of Evolution call this layer for a JMAP
// account. `tests/create_resource.rs` and `tests/delete_resource.rs` test it
// only against `jmap-mockd`. This file tests it against a real server.
//
// The test is skipped, not failed, when `JMAP_LIVE_SERVER_WRITE_USER` or
// `_PASSWORD` is not set. See docs/manual-test-live-server.md.

use std::env;
use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use eds_sys::{
    ESource, ESourceRegistryServer, e_server_side_source_get_write_directory,
    e_server_side_source_new, e_source_address_book_get_type, e_source_calendar_get_type,
    e_source_get_parent, e_source_get_writable, e_source_registry_server_new, g_file_new_for_path,
};
use glib_sys::GFALSE;
use gobject_sys::{GObject, g_object_unref};
use jmap_backend_collection::child_source::apply;
use jmap_backend_collection::create_resource::{adopt_created, create_on_server};
use jmap_backend_collection::delete_resource::{delete_on_server, doomed_of};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::source::ConnectTarget;
use jmap_client::Credentials;
use jmap_collection_sync::child_source::Connection;
use jmap_collection_sync::{ChildKind, Fanout, Parts, Requested};
use jmap_test_support::{connect_for_write, unique_suffix};

static NEXT: AtomicU32 = AtomicU32::new(0);

/// The server address of the account of this test. It is intentionally not
/// the address of the real server, as in `connection()` of
/// `tests/live_server.rs`. A child must use the connection of the login, not
/// the address of the create.
fn connection() -> Connection {
    Connection {
        host: "jmap.example.com".to_owned(),
        port: Some(8443),
        user: Some("agent1@agent-backendresource.test".to_owned()),
        auth_method: Some("plain/password".to_owned()),
        secure: true,
    }
}

/// A real, empty `EServerSideSource`, as the `remote_create` handler of EDS and
/// `child_added` give to this backend. This runner has no
/// `evolution-source-registry` daemon. The construction is the same as in
/// `tests/create_resource.rs` and `tests/delete_resource.rs`.
struct ServerSide {
    /// Kept for the life of the source. `e_server_side_source_new` keeps only a
    /// weak reference to the server.
    server: *mut ESourceRegistryServer,
    source: *mut ESource,
}

impl ServerSide {
    fn new() -> Self {
        // SAFETY: these functions take no arguments. `e_source_get_extension`
        // cannot find an extension class whose type is not yet referenced.
        unsafe {
            e_source_address_book_get_type();
            e_source_calendar_get_type();
        }

        let path = std::env::temp_dir().join(format!(
            "jmap-live-resource-{}-{}.source",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let path = CString::new(path.into_os_string().into_encoded_bytes())
            .expect("no NUL in a temp path");
        let mut error = ptr::null_mut();

        // SAFETY: the constructor takes no arguments. The path is
        // NUL-terminated and the callee copies it. This code owns the GFile and
        // releases it below.
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
            // SAFETY: a NULL return means the GError was set.
            unsafe { CStr::from_ptr((*error).message) }.to_string_lossy()
        );

        Self { server, source }
    }

    fn parent(&self) -> Option<String> {
        // SAFETY: a live source.
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
}

impl Drop for ServerSide {
    fn drop(&mut self) {
        // SAFETY: this struct holds the only reference to each object. The
        // source is released first because it holds a weak reference to the
        // server.
        unsafe {
            g_object_unref(self.source.cast());
            g_object_unref(self.server.cast::<GObject>());
        }
    }
}

/// Creates a real address book through `create_on_server`/`adopt_created`.
/// Then the test makes sure that an independent connection sees it. It deletes
/// the address book through `delete_on_server`, as `delete_resource_sync` does
/// for the child source of a `child_added`. Last, it makes sure that the
/// independent connection no longer sees it.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn create_resource_then_delete_resource_round_trip_through_the_real_server() {
    let Some(verify) = connect_for_write() else {
        eprintln!("JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the write-path test");
        return;
    };
    let user = env::var("JMAP_LIVE_SERVER_WRITE_USER").expect("connect_for_write succeeded");
    let password =
        env::var("JMAP_LIVE_SERVER_WRITE_PASSWORD").expect("connect_for_write succeeded");
    let origin = env::var("JMAP_LIVE_SERVER_URL").expect("connect_for_write succeeded");
    let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");
    let target = ConnectTarget::Origin(origin);

    let name = format!("agent-resource-{}", unique_suffix());
    let scratch = ServerSide::new();

    let created = create_on_server(
        &target,
        rebase,
        Credentials::basic(user.clone(), password.clone()),
        &Requested {
            kind: ChildKind::AddressBook,
            display_name: name,
        },
    )
    .expect("AddressBook/set create failed against the real server");

    // SAFETY: `scratch.source` is a live `EServerSideSource` that this test
    // holds a reference to.
    unsafe {
        adopt_created(
            scratch.source,
            &created,
            &connection(),
            "agent-jmap-account",
            Some("/var/lib/agent-jmap-cache"),
        )
    }
    .expect("every setting of a child this backend describes is writable");

    assert_eq!(scratch.parent().as_deref(), Some("agent-jmap-account"));
    assert!(scratch.writable());
    assert_eq!(
        scratch.write_directory().as_deref(),
        Some("/var/lib/agent-jmap-cache")
    );

    let fanout = Fanout::discover(&verify, Parts::ALL)
        .expect("an independent connection should be able to discover the real account");
    assert!(
        fanout
            .address_books
            .iter()
            .any(|book| book.id == created.collection_id),
        "the address book create_on_server just made should be visible over an \
         independent connection"
    );

    // The child that a `child_added` for this collection gets. Its settings
    // come from the same `Child` as the settings of `scratch` above. Thus
    // `delete_resource_sync` reads the collection id from `create_on_server`.
    let doomed_source = ServerSide::new();
    // SAFETY: a live source this test holds a reference to.
    unsafe { apply(doomed_source.source, &created.settings(&connection())) }
        .expect("a child this backend wrote");
    let doomed =
        // SAFETY: as above.
        unsafe { doomed_of(doomed_source.source) }.expect("a source written from create_on_server's own Child");

    delete_on_server(&target, rebase, Credentials::basic(user, password), &doomed)
        .expect("AddressBook/set destroy failed against the real server");

    let fanout = Fanout::discover(&verify, Parts::ALL)
        .expect("an independent connection should still be able to discover the real account");
    assert!(
        !fanout
            .address_books
            .iter()
            .any(|book| book.id == created.collection_id),
        "the address book delete_on_server destroyed should be gone from an \
         independent connection"
    );
}
