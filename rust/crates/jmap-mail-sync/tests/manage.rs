// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Creation, deletion and rename of folders by the user, as a `Mailbox/set`.
//!
//! Camel uses three vfuncs, and RFC 8621 §2.5 has one method for all three:
//!
//! - `create_folder_sync` returns the new folder.
//! - `delete_folder_sync` tells whether the folder is gone.
//! - `rename_folder_sync` is a rename and a move, because Camel gives both as
//!   a new path.
//!
//! The tests check three rules:
//!
//! * **The result is a folder, not an id.** Camel gives the new
//!   `CamelFolderInfo` directly to the folder tree, so the result must include
//!   the Camel *path*. This crate makes the path from the parent path and an
//!   encoded name. With only an id, the caller must list the account again.
//! * **A refusal keeps the server reason.** `mailboxHasChild` and
//!   `mailboxHasEmail` (RFC 8621 §2.5) are refusals, not failures. The user
//!   needs a different message for each, so this crate keeps them apart.
//! * **A refusal changes nothing.** A refused create must not add a folder,
//!   and a refused destroy must not remove one.

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{FolderInfo, MailSync, SyncError};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use jmap_proto::error::set::{INVALID_PROPERTIES, NOT_FOUND};
use jmap_proto::mail::mailbox_set_error::{HAS_CHILD, HAS_EMAIL};

struct Fixture {
    server: MockServer,
    account_id: Id,
}

impl Fixture {
    fn start() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        Self { server, account_id }
    }

    fn sync(&self) -> MailSync {
        let client = Client::connect(self.server.origin(), Credentials::none()).unwrap();
        MailSync::new(client, self.account_id.clone())
    }

    /// Seed a top-level mailbox with the given name.
    fn seed_mailbox(&self, name: &str) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&self.account_id)
            .unwrap()
            .seed_mailbox(name, None)
    }

    /// Seed a mailbox with the given name below `parent`.
    fn seed_child_mailbox(&self, name: &str, parent: &Id) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&self.account_id)
            .unwrap()
            .seed_child_mailbox(name, None, parent)
    }

    /// Seed a message in a mailbox, so that the server refuses its deletion.
    fn seed_email(&self, mailbox: &Id) {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&self.account_id)
            .unwrap()
            .seed_email(EmailSeed::new(
                mailbox.clone(),
                ("Bob", "bob@example.com"),
                "Lunch?",
                "text",
                "2026-01-15T09:00:00Z",
            ));
    }

    /// The folder at `path` in a new listing.
    fn folder(&self, path: &str) -> Option<FolderInfo> {
        let (_, tree) = self.sync().folder_tree().unwrap();
        tree.find(path).cloned()
    }

    /// The number of folders in the account.
    fn folder_count(&self) -> usize {
        let (_, tree) = self.sync().folder_tree().unwrap();
        tree.len()
    }
}

/// The `SetError` type of a refusal. Panics with the result if it is not one.
fn refusal(failure: SyncError) -> String {
    match failure {
        SyncError::Client(jmap_client::Error::Set(error)) => error.error_type,
        other => panic!("expected the server's own refusal, got {other}"),
    }
}

// ---------------------------------------------------------------------------
// create

#[test]
fn a_created_folder_reaches_the_server() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Inbox");

    fixture.sync().create_folder(None, "Projects").unwrap();

    assert!(fixture.folder("Projects").is_some());
}

/// Camel finds a folder by its path. Without the path, the caller cannot open
/// the new folder.
#[test]
fn the_answer_is_the_folder_as_camel_asks_for_it() {
    let fixture = Fixture::start();

    let created = fixture.sync().create_folder(None, "Projects").unwrap();

    assert_eq!(created.path, "Projects");
    assert_eq!(created.display_name, "Projects");
    assert_eq!(
        fixture.folder("Projects").map(|folder| folder.id),
        Some(created.id)
    );
}

/// The result shows the folder as it *is*: a new mailbox has no contents yet.
#[test]
fn a_new_folder_is_empty_and_childless() {
    let fixture = Fixture::start();

    let created = fixture.sync().create_folder(None, "Projects").unwrap();

    assert_eq!(created.total, 0);
    assert_eq!(created.unread, 0);
    assert!(created.children.is_empty());
    assert_eq!(created.role, None);
}

/// The user wants to see a new folder, so it must be subscribed before the
/// next listing. Else, Evolution hides the new folder.
#[test]
fn a_new_folder_is_subscribed() {
    let fixture = Fixture::start();

    let created = fixture.sync().create_folder(None, "Projects").unwrap();

    assert!(created.subscribed);
    assert_eq!(
        fixture.folder("Projects").map(|folder| folder.subscribed),
        Some(true)
    );
}

/// Below a parent, the path must agree with the listing, because the caller
/// opens the new folder by this string.
#[test]
fn a_child_hangs_under_its_parent() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Work");
    let parent = fixture.folder("Work").unwrap();

    let created = fixture
        .sync()
        .create_folder(Some(&parent), "Notes")
        .unwrap();

    assert_eq!(created.path, "Work/Notes");
    assert_eq!(
        fixture.folder("Work/Notes").map(|folder| folder.id),
        Some(created.id)
    );
}

/// A mailbox name is a display string, and a Camel path component is an
/// identifier. The result must use the same encoding as a listing. Else, the
/// next listing shows a different folder.
#[test]
fn a_name_that_is_not_a_path_component_is_encoded() {
    let fixture = Fixture::start();

    let created = fixture.sync().create_folder(None, "and/or").unwrap();

    assert_eq!(created.path, "and%2For");
    assert_eq!(created.display_name, "and/or");
    assert_eq!(
        fixture.folder("and%2For").map(|folder| folder.id),
        Some(created.id)
    );
}

/// RFC 8621 §2 makes a mailbox name unique among its siblings. Only the server
/// can check this, and the user must see its reason.
#[test]
fn a_name_a_sibling_already_has_is_refused() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Projects");
    let before = fixture.folder_count();

    let failure = fixture.sync().create_folder(None, "Projects").unwrap_err();

    assert_eq!(refusal(failure), INVALID_PROPERTIES);
    assert_eq!(fixture.folder_count(), before);
}

// ---------------------------------------------------------------------------
// delete

#[test]
fn a_deleted_folder_is_gone_from_the_account() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Inbox");
    let doomed = fixture.seed_mailbox("Projects");

    fixture.sync().delete_folder(&doomed).unwrap();

    assert!(fixture.folder("Projects").is_none());
    assert!(fixture.folder("Inbox").is_some());
}

/// The refusal for a folder that contains mail. The user decides about the
/// mail, so this client sends no `onDestroyRemoveEmails`. The folder stays,
/// and the user must see the reason.
#[test]
fn a_folder_that_still_holds_mail_is_refused() {
    let fixture = Fixture::start();
    let full = fixture.seed_mailbox("Projects");
    fixture.seed_email(&full);

    let failure = fixture.sync().delete_folder(&full).unwrap_err();

    assert_eq!(refusal(failure), HAS_EMAIL);
    assert!(fixture.folder("Projects").is_some());
}

/// The other refusal, with a different message for the user: the folder has
/// no mail, but it has subfolders.
#[test]
fn a_folder_that_still_holds_a_subfolder_is_refused() {
    let fixture = Fixture::start();
    let parent = fixture.seed_mailbox("Work");
    let child = fixture.seed_child_mailbox("Notes", &parent);

    let failure = fixture.sync().delete_folder(&parent).unwrap_err();

    assert_eq!(refusal(failure), HAS_CHILD);
    assert!(fixture.folder("Work").is_some());
    assert_eq!(
        fixture.folder("Work/Notes").map(|folder| folder.id),
        Some(child)
    );
}

/// A different client removed the folder, but this client still lists it. The
/// folder is already gone, as the user wants. Thus the result is the variant
/// that the layer above maps to `CAMEL_STORE_ERROR_NO_FOLDER`, as for an
/// unsubscribe of a missing folder.
#[test]
fn a_mailbox_the_account_does_not_have_is_no_such_folder() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Inbox");

    let failure = fixture.sync().delete_folder(&Id::new("M404")).unwrap_err();

    match failure {
        SyncError::NoSuchFolder(id) => assert_eq!(id.as_str(), "M404"),
        other => panic!("expected a missing folder, got {other}"),
    }
}

/// The `notFound` mapping applies only to *this* refusal. The other refusals
/// stay unchanged. Else, Evolution removes a folder that still exists.
#[test]
fn the_other_refusals_are_not_read_as_a_missing_folder() {
    let fixture = Fixture::start();
    let full = fixture.seed_mailbox("Projects");
    fixture.seed_email(&full);

    let failure = fixture.sync().delete_folder(&full).unwrap_err();

    assert_ne!(refusal(failure), NOT_FOUND);
}

// ---------------------------------------------------------------------------
// rename (which is also move)

#[test]
fn a_renamed_folder_answers_to_its_new_name() {
    let fixture = Fixture::start();
    let folder = fixture.seed_mailbox("Projects");

    fixture.sync().rename_folder(&folder, None, "Work").unwrap();

    assert_eq!(fixture.folder("Work").map(|folder| folder.id), Some(folder));
    assert!(fixture.folder("Projects").is_none());
}

/// The result is the path, as for a create. The caller finds the folder by its
/// path but cannot make it, because the name encoding is in this crate.
#[test]
fn the_answer_is_the_path_the_folder_now_has() {
    let fixture = Fixture::start();
    let folder = fixture.seed_mailbox("Projects");

    let path = fixture.sync().rename_folder(&folder, None, "Work").unwrap();

    assert_eq!(path, "Work");
}

/// Camel gives a move as a rename to a path below a different parent. Thus
/// both are one write, and the result starts with the new parent path.
#[test]
fn a_moved_folder_hangs_under_its_new_parent() {
    let fixture = Fixture::start();
    let folder = fixture.seed_mailbox("Notes");
    fixture.seed_mailbox("Work");
    let parent = fixture.folder("Work").unwrap();

    let path = fixture
        .sync()
        .rename_folder(&folder, Some(&parent), "Notes")
        .unwrap();

    assert_eq!(path, "Work/Notes");
    assert_eq!(
        fixture.folder("Work/Notes").map(|folder| folder.id),
        Some(folder)
    );
}

/// A move to the top level. This proves that the patch *sends* a `null`
/// `parentId`. Without it, the folder does not move, and the result is a
/// path with no folder.
#[test]
fn a_folder_moved_to_the_top_level_loses_its_parent() {
    let fixture = Fixture::start();
    let parent = fixture.seed_mailbox("Work");
    let folder = fixture.seed_child_mailbox("Notes", &parent);

    let path = fixture
        .sync()
        .rename_folder(&folder, None, "Notes")
        .unwrap();

    assert_eq!(path, "Notes");
    assert_eq!(
        fixture.folder("Notes").map(|folder| folder.id),
        Some(folder)
    );
}

/// The new name is a mailbox name, and the result is a Camel path. The same
/// encoding as for a create applies.
#[test]
fn a_new_name_that_is_not_a_path_component_is_encoded() {
    let fixture = Fixture::start();
    let folder = fixture.seed_mailbox("Projects");

    let path = fixture
        .sync()
        .rename_folder(&folder, None, "and/or")
        .unwrap();

    assert_eq!(path, "and%2For");
    assert_eq!(
        fixture.folder("and%2For").map(|folder| folder.id),
        Some(folder)
    );
}

/// RFC 8621 §2 makes a name unique among siblings, and a rename can also break
/// this rule. The folder does not change.
#[test]
fn a_new_name_a_sibling_already_has_is_refused() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Work");
    let folder = fixture.seed_mailbox("Projects");

    let failure = fixture
        .sync()
        .rename_folder(&folder, None, "Work")
        .unwrap_err();

    assert_eq!(refusal(failure), INVALID_PROPERTIES);
    assert_eq!(
        fixture.folder("Projects").map(|folder| folder.id),
        Some(folder)
    );
}

/// The server refuses a move of a folder into its own subtree. Such a subtree
/// has no connection to the account root.
#[test]
fn a_move_into_the_folders_own_subtree_is_refused() {
    let fixture = Fixture::start();
    let folder = fixture.seed_mailbox("Work");
    fixture.seed_child_mailbox("Notes", &folder);
    let child = fixture.folder("Work/Notes").unwrap();

    let failure = fixture
        .sync()
        .rename_folder(&folder, Some(&child), "Work")
        .unwrap_err();

    assert_eq!(refusal(failure), INVALID_PROPERTIES);
    assert_eq!(fixture.folder("Work").map(|folder| folder.id), Some(folder));
}

/// A different client removed the folder, but this client still lists it. The
/// result is the same variant as for the other mailbox writes.
#[test]
fn renaming_a_mailbox_the_account_does_not_have_is_no_such_folder() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Inbox");

    let failure = fixture
        .sync()
        .rename_folder(&Id::new("M404"), None, "Work")
        .unwrap_err();

    match failure {
        SyncError::NoSuchFolder(id) => assert_eq!(id.as_str(), "M404"),
        other => panic!("expected a missing folder, got {other}"),
    }
}
