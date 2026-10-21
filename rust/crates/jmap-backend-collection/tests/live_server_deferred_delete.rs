// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Tests the undo window of a delete against a real JMAP server.
//
// The owner shares calendars with the recipient. The recipient deletes them
// through `defer` and `watch`, as `delete_resource_sync` does. The tests make
// sure that the server destroys nothing before the due time, that an undo
// keeps the calendar for the owner, and that the destroy happens after the
// due time. `tests/deferred_delete.rs` tests the same code with a fake server.
//
// The tests are skipped, not failed, when the write or the recipient account is
// not set. Refer to docs/manual-test-live-server.md.

use std::env;
use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, SystemTime};

use eds_sys::{
    ESource, ESourceRegistryServer, e_server_side_source_new, e_source_get_enabled,
    e_source_get_uid, e_source_registry_server_new, e_source_set_enabled, g_file_new_for_path,
};
use glib_sys::{GFALSE, GTRUE};
use gobject_sys::{GObject, g_object_unref};
use jmap_backend_collection::child_source::apply;
use jmap_backend_collection::deferred_delete::{PendingChild, Settled, defer, watch};
use jmap_backend_collection::delete_resource::{DeleteError, delete_on_server, doomed_of};
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::source::ConnectTarget;
use jmap_client::{Client, Credentials};
use jmap_collection_sync::child_source::Connection;
use jmap_collection_sync::pending::{Due, UNDO_WINDOW};
use jmap_collection_sync::{Fanout, Parts};
use jmap_proto::Id;
use jmap_proto::calendars::Calendar;
use jmap_proto::principals::PrincipalQueryFilter;
use jmap_proto::session::{CAPABILITY_CALENDARS, CAPABILITY_PRINCIPALS};
use serde_json::json;

static NEXT: AtomicU32 = AtomicU32::new(0);

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// The login of one account, from the environment.
struct Account {
    user: String,
    password: String,
    origin: String,
    rebase: bool,
}

impl Account {
    fn from_env(prefix: &str) -> Option<Self> {
        let user = env::var(format!("{prefix}_USER")).ok()?;
        let password = env::var(format!("{prefix}_PASSWORD"))
            .unwrap_or_else(|_| panic!("{prefix}_USER is set but {prefix}_PASSWORD is not"));
        let origin = env::var("JMAP_LIVE_SERVER_URL")
            .unwrap_or_else(|_| panic!("set JMAP_LIVE_SERVER_URL alongside {prefix}_USER"));
        let rebase = env::var("JMAP_LIVE_SERVER_REBASE_URLS").is_ok_and(|value| value != "0");
        Some(Self {
            user,
            password,
            origin,
            rebase,
        })
    }

    fn credentials(&self) -> Credentials {
        Credentials::basic(self.user.clone(), self.password.clone())
    }

    fn connect(&self) -> Client {
        Client::builder()
            .rebase_urls_to_origin(self.rebase)
            .connect(&self.origin, self.credentials())
            .expect("could not fetch the session document")
    }
}

/// A connection that is not the address of the real server. The destroy must
/// use the login of the account, not the settings of the child.
fn connection() -> Connection {
    Connection {
        host: "jmap.example.com".to_owned(),
        port: Some(8443),
        user: Some("agent-recipient@agent-deferred.test".to_owned()),
        auth_method: Some("plain/password".to_owned()),
        secure: true,
    }
}

/// A real `EServerSideSource`, as `child_added` gives to the backend. The
/// construction is the same as in `tests/live_server_resource.rs`.
struct ServerSide {
    /// `e_server_side_source_new` keeps only a weak reference to the server.
    server: *mut ESourceRegistryServer,
    source: *mut ESource,
}

// SAFETY: the reference counts of GObject are atomic, and the watch thread
// only reads and sets properties, as the registry does.
unsafe impl Send for ServerSide {}
unsafe impl Sync for ServerSide {}

impl ServerSide {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jmap-live-deferred-{}-{}.source",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let path = CString::new(path.into_os_string().into_encoded_bytes())
            .expect("no NUL in a temp path");
        let mut error = ptr::null_mut();
        // SAFETY: the constructor takes no arguments. The path is
        // NUL-terminated and the callee copies it. The test releases the GFile.
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
            // SAFETY: a NULL return means the GError is set.
            unsafe { CStr::from_ptr((*error).message) }.to_string_lossy()
        );
        Self { server, source }
    }

    fn enabled(&self) -> bool {
        // SAFETY: a live source.
        unsafe { e_source_get_enabled(self.source) != GFALSE }
    }

    fn due(&self) -> Option<Due> {
        // SAFETY: a live source.
        unsafe { jmap_config::pending_delete::read(self.source) }
    }

    /// What the user does with "Undo".
    fn undo(&self) {
        // SAFETY: a live source.
        unsafe { e_source_set_enabled(self.source, GTRUE) };
    }
}

impl Drop for ServerSide {
    fn drop(&mut self) {
        // SAFETY: the only references. The source holds a weak reference to
        // the server, so it is released first.
        unsafe {
            g_object_unref(self.source.cast());
            g_object_unref(self.server.cast::<GObject>());
        }
    }
}

/// The child of the recipient, with the real destroy of `delete_on_server`.
///
/// The registry is not running here, so `remove` only records the call.
struct RecipientChild {
    side: Arc<ServerSide>,
    target: ConnectTarget,
    rebase: bool,
    credentials: Credentials,
    removed: Arc<AtomicBool>,
}

impl PendingChild for RecipientChild {
    fn uid(&self) -> String {
        // SAFETY: a live source. The uid is `(transfer none)`.
        unsafe { read_string(e_source_get_uid(self.side.source)) }.unwrap_or_default()
    }

    fn present(&self) -> bool {
        true
    }

    fn enabled(&self) -> bool {
        self.side.enabled()
    }

    fn due(&self) -> Option<Due> {
        self.side.due()
    }

    fn clear_due(&self) {
        // SAFETY: a live source.
        unsafe { jmap_config::pending_delete::write(self.side.source, None) };
    }

    fn enable(&self) {
        self.side.undo();
    }

    fn destroy_on_server(&self) -> Result<(), DeleteError> {
        // SAFETY: a live source.
        let doomed = unsafe { doomed_of(self.side.source) }.ok_or(DeleteError::NotOurs)?;
        delete_on_server(&self.target, self.rebase, self.credentials.clone(), &doomed)
    }

    fn remove(&self) -> Result<(), String> {
        self.removed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

/// The owner, a calendar that the owner shares with the recipient, and the
/// child source of the recipient for it.
struct Shared {
    owner: Client,
    owner_account: Id,
    calendar_id: Id,
    recipient: Account,
    side: Arc<ServerSide>,
}

impl Shared {
    /// Returns `None` (and the test skips) without the two accounts.
    fn new(name: &str, rights: serde_json::Value) -> Option<Self> {
        let owner_login = Account::from_env("JMAP_LIVE_SERVER_WRITE")?;
        let recipient = Account::from_env("JMAP_LIVE_SERVER_RECIPIENT")?;
        let owner = owner_login.connect();
        if !owner
            .session()
            .capabilities
            .contains_key(CAPABILITY_PRINCIPALS)
        {
            eprintln!("the server does not advertise {CAPABILITY_PRINCIPALS}");
            return None;
        }
        let owner_account = owner
            .primary_account(CAPABILITY_CALENDARS)
            .expect("the write-test account needs the calendars capability");
        let principal = owner
            .principal_query(&owner_account, PrincipalQueryFilter::email(&recipient.user))
            .expect("Principal/query failed against the real server")
            .into_iter()
            .next()
            .expect("the owner cannot find the principal of the recipient");
        let calendar_id = owner
            .calendar_create(
                &owner_account,
                &Calendar::new(format!("agent-deferred-{name}-{}", unique_suffix())),
            )
            .expect("Calendar/set create failed against the real server")
            .id
            .expect("the server names the new calendar");
        owner
            .calendar_update(
                &owner_account,
                &calendar_id,
                json!({"shareWith": {principal.as_str(): rights}}),
            )
            .expect("Calendar/set shareWith failed against the real server");

        let resource_id = format!("calendar:{owner_account}/{calendar_id}");
        let child = Fanout::discover(&recipient.connect(), Parts::ALL)
            .expect("discovery failed for the recipient")
            .children()
            .into_iter()
            .find(|child| child.resource_id == resource_id)
            .unwrap_or_else(|| panic!("the recipient has no child {resource_id}"));
        let side = Arc::new(ServerSide::new());
        // SAFETY: a live source that this test holds a reference to.
        unsafe { apply(side.source, &child.settings(&connection())) }
            .expect("a child that this backend describes");

        Some(Self {
            owner,
            owner_account,
            calendar_id,
            recipient,
            side,
        })
    }

    fn child(&self, removed: &Arc<AtomicBool>) -> RecipientChild {
        RecipientChild {
            side: Arc::clone(&self.side),
            target: ConnectTarget::Origin(self.recipient.origin.clone()),
            rebase: self.recipient.rebase,
            credentials: self.recipient.credentials(),
            removed: Arc::clone(removed),
        }
    }

    /// Reads the account of the owner, not a discovery of the recipient.
    fn owner_has_it(&self) -> bool {
        self.owner
            .calendars(&self.owner_account)
            .expect("Calendar/get failed for the owner")
            .iter()
            .any(|calendar| calendar.id.as_ref() == Some(&self.calendar_id))
    }

    fn recipient_has_it(&self) -> bool {
        let resource_id = format!("calendar:{}/{}", self.owner_account, self.calendar_id);
        Fanout::discover(&self.recipient.connect(), Parts::ALL)
            .expect("discovery failed for the recipient")
            .children()
            .iter()
            .any(|child| child.resource_id == resource_id)
    }

    /// Destroys the calendar if it is still there, so a failed assert leaves
    /// nothing behind.
    fn clean_up(&self) {
        if self.owner_has_it() {
            self.owner
                .calendar_destroy(&self.owner_account, &self.calendar_id)
                .expect("Calendar/set destroy failed (cleanup)");
        }
    }
}

/// A `now` that makes the due time `left` from the real now.
fn due_in(left: Duration) -> SystemTime {
    SystemTime::now() - (UNDO_WINDOW - left)
}

const MAY_DELETE: fn() -> serde_json::Value =
    || json!({"mayReadItems": true, "mayReadFreeBusy": true, "mayDelete": true});

/// The full undo window: the calendar stays on the server until the due time,
/// and then the recipient destroys it for the owner too.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_shared_calendar_is_destroyed_for_the_owner_only_after_the_undo_window() {
    let Some(shared) = Shared::new("destroy", MAY_DELETE()) else {
        eprintln!("the write or the recipient account is not set; skipping");
        return;
    };
    let removed = Arc::new(AtomicBool::new(false));

    // SAFETY: a live source.
    let due = unsafe { defer(shared.side.source, SystemTime::now()) };
    let handle = watch(shared.child(&removed)).expect("no other thread watches this child");

    let mut seen_before_due = 0;
    loop {
        let left = due.stamp()
            - SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
        if left <= 2 {
            break;
        }
        assert!(!shared.side.enabled(), "the deleted child is hidden");
        let (owner, recipient) = (shared.owner_has_it(), shared.recipient_has_it());
        if !owner || !recipient {
            shared.clean_up();
            panic!("destroyed {left} s before the due time (owner {owner}, recipient {recipient})");
        }
        seen_before_due += 1;
        std::thread::sleep(Duration::from_secs(10));
    }
    let settled = handle.join().expect("the watch thread did not panic");
    let owner_after = shared.owner_has_it();
    shared.clean_up();

    assert!(seen_before_due >= 5, "checked {seen_before_due} times");
    assert_eq!(settled, Settled::Destroyed);
    assert!(
        !owner_after,
        "the owner still has the calendar after the due time"
    );
    assert!(removed.load(Ordering::SeqCst), "the source is removed");
}

/// An undo inside the window sends nothing: the calendar stays for both users.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn an_undo_keeps_a_shared_calendar_for_the_owner_and_the_recipient() {
    let Some(shared) = Shared::new("undo", MAY_DELETE()) else {
        eprintln!("the write or the recipient account is not set; skipping");
        return;
    };
    let removed = Arc::new(AtomicBool::new(false));

    // SAFETY: a live source.
    unsafe { defer(shared.side.source, due_in(Duration::from_secs(5))) };
    let handle = watch(shared.child(&removed)).expect("no other thread watches this child");
    let hidden = !shared.side.enabled();
    std::thread::sleep(Duration::from_secs(2));
    let kept_before_undo = shared.owner_has_it();
    shared.side.undo();
    let settled = handle.join().expect("the watch thread did not panic");
    let (owner, recipient) = (shared.owner_has_it(), shared.recipient_has_it());
    let due_left = shared.side.due();
    shared.clean_up();

    assert!(hidden, "the deleted child is hidden until the undo");
    assert!(kept_before_undo, "destroyed before the due time");
    assert_eq!(settled, Settled::Undone);
    assert!(owner, "the owner lost the calendar after an undo");
    assert!(recipient, "the recipient lost the calendar after an undo");
    assert_eq!(due_left, None);
    assert!(!removed.load(Ordering::SeqCst));
}

/// A share without `mayDelete`: the server refuses the destroy at the due
/// time, and the child is shown again.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_refused_destroy_shows_the_shared_calendar_again() {
    let Some(shared) = Shared::new("refused", json!({"mayReadItems": true})) else {
        eprintln!("the write or the recipient account is not set; skipping");
        return;
    };
    let removed = Arc::new(AtomicBool::new(false));

    // SAFETY: a live source.
    unsafe { defer(shared.side.source, due_in(Duration::from_secs(3))) };
    let settled = watch(shared.child(&removed))
        .expect("no other thread watches this child")
        .join()
        .expect("the watch thread did not panic");
    let owner = shared.owner_has_it();
    shared.clean_up();

    assert_eq!(settled, Settled::Restored);
    assert!(
        owner,
        "the server destroyed a calendar shared without mayDelete"
    );
    assert!(shared.side.enabled(), "the child is shown again");
    assert_eq!(shared.side.due(), None);
    assert!(!removed.load(Ordering::SeqCst));
}
