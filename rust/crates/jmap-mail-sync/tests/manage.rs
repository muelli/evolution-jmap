// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The folders the user makes, removes and renames, as the `Mailbox/set` that
//! does it.
//!
//! Camel asks with three vfuncs — `create_folder_sync`, which must answer with
//! the folder it made, `delete_folder_sync`, which must answer whether the
//! folder went, and `rename_folder_sync`, which is a rename and a move in one
//! because Camel spells both as a new path — and RFC 8621 §2.5 answers all
//! three with one method.
//!
//! Three things are worth pinning here, and none of them is "the request was
//! sent":
//!
//! * **The answer is a folder, not an id.** Camel hands the new
//!   `CamelFolderInfo` straight to the folder tree, so the Camel *path* — this
//!   crate's own invention, built out of the parent's path and an encoded
//!   name — has to come back with it. A create that answered with an id would
//!   make the caller re-list the account to learn a path it already knows.
//! * **A refusal keeps the server's reason.** RFC 8621 §2.5's
//!   `mailboxHasChild` and `mailboxHasEmail` are the server declining rather
//!   than failing, and what the user has to be told differs between them —
//!   so the distinction has to survive this crate.
//! * **A refusal changes nothing.** A rejected create must not leave a folder
//!   behind and a rejected destroy must not take one away.

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

    /// A top-level mailbox of the account, by name.
    fn seed_mailbox(&self, name: &str) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&self.account_id)
            .unwrap()
            .seed_mailbox(name, None)
    }

    /// A mailbox under another, by name.
    fn seed_child_mailbox(&self, name: &str, parent: &Id) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        state
            .account_mut(&self.account_id)
            .unwrap()
            .seed_child_mailbox(name, None, parent)
    }

    /// A message sitting in a mailbox, so that removing it is refused.
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

    /// The account's folder tree, as a fresh listing reports it.
    fn folder(&self, path: &str) -> Option<FolderInfo> {
        let (_, tree) = self.sync().folder_tree().unwrap();
        tree.find(path).cloned()
    }

    /// How many folders the account has at all.
    fn folder_count(&self) -> usize {
        let (_, tree) = self.sync().folder_tree().unwrap();
        tree.len()
    }
}

/// The `SetError` type a refusal carries, or a panic naming what came instead.
fn refusal(failure: SyncError) -> String {
    match failure {
        SyncError::Client(jmap_client::Error::Set(error)) => error.error_type,
        other => panic!("expected the server's own refusal, got {other}"),
    }
}

// ---------------------------------------------------------------------------
// making one

#[test]
fn a_created_folder_reaches_the_server() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Inbox");

    fixture.sync().create_folder(None, "Projects").unwrap();

    assert!(fixture.folder("Projects").is_some());
}

/// The path is what Camel keys the folder by, so a create that could not
/// answer with one would be a folder the caller cannot open.
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

/// And it is the folder as it *is*: nothing has had a chance to arrive in a
/// mailbox that did not exist a moment ago.
#[test]
fn a_new_folder_is_empty_and_childless() {
    let fixture = Fixture::start();

    let created = fixture.sync().create_folder(None, "Projects").unwrap();

    assert_eq!(created.total, 0);
    assert_eq!(created.unread, 0);
    assert!(created.children.is_empty());
    assert_eq!(created.role, None);
}

/// A folder the user has just asked for is one they want to see, and the
/// server says so: the tick has to be on before the next listing, or Evolution
/// hides the folder it was told to make.
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

/// Under a parent, the two halves of the path have to agree with the listing's
/// — the caller opens the new folder by exactly this string.
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

/// A mailbox name is a display string and a Camel path component is an
/// identifier, so the answer has to encode the one into the other — the same
/// mapping a listing makes, or the folder the create answered with is not the
/// folder the next listing describes.
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

/// RFC 8621 §2 makes a mailbox name unique among its siblings, and the server
/// is the only side that can say so. The reason is the user's to read.
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
// removing one

#[test]
fn a_deleted_folder_is_gone_from_the_account() {
    let fixture = Fixture::start();
    fixture.seed_mailbox("Inbox");
    let doomed = fixture.seed_mailbox("Projects");

    fixture.sync().delete_folder(&doomed).unwrap();

    assert!(fixture.folder("Projects").is_none());
    assert!(fixture.folder("Inbox").is_some());
}

/// The refusal a folder with mail in it gets. It is the server declining
/// rather than failing — what becomes of the mail is the user's decision, and
/// this client sends no `onDestroyRemoveEmails` — so the folder is still there
/// afterwards and the reason is the one the user has to be shown.
#[test]
fn a_folder_that_still_holds_mail_is_refused() {
    let fixture = Fixture::start();
    let full = fixture.seed_mailbox("Projects");
    fixture.seed_email(&full);

    let failure = fixture.sync().delete_folder(&full).unwrap_err();

    assert_eq!(refusal(failure), HAS_EMAIL);
    assert!(fixture.folder("Projects").is_some());
}

/// And the other one, which is a different sentence to the user: the folder is
/// empty of mail and still has folders under it.
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

/// A folder another client removed while this one still lists it. Deleting
/// what is already gone is what the user asked for, and reporting it as a
/// broken account would turn someone else's tidying into an alert — so it
/// keeps the variant the layer above maps onto `CAMEL_STORE_ERROR_NO_FOLDER`,
/// exactly as an unsubscribe of a vanished folder does.
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

/// The `notFound` mapping is a judgement about *this* refusal only. Everything
/// else the server says stays its own, so that the two refusals above are not
/// quietly turned into "that folder is gone" — which would have Evolution
/// remove a folder that is still there.
#[test]
fn the_other_refusals_are_not_read_as_a_missing_folder() {
    let fixture = Fixture::start();
    let full = fixture.seed_mailbox("Projects");
    fixture.seed_email(&full);

    let failure = fixture.sync().delete_folder(&full).unwrap_err();

    assert_ne!(refusal(failure), NOT_FOUND);
}

// ---------------------------------------------------------------------------
// renaming one, which is also moving one

#[test]
fn a_renamed_folder_answers_to_its_new_name() {
    let fixture = Fixture::start();
    let folder = fixture.seed_mailbox("Projects");

    fixture.sync().rename_folder(&folder, None, "Work").unwrap();

    assert_eq!(fixture.folder("Work").map(|folder| folder.id), Some(folder));
    assert!(fixture.folder("Projects").is_none());
}

/// The answer is the path, for the reason a create answers with a whole folder:
/// the caller keys the folder by it and cannot build it, because the encoding
/// from a mailbox name to a path component lives in this crate.
#[test]
fn the_answer_is_the_path_the_folder_now_has() {
    let fixture = Fixture::start();
    let folder = fixture.seed_mailbox("Projects");

    let path = fixture.sync().rename_folder(&folder, None, "Work").unwrap();

    assert_eq!(path, "Work");
}

/// Camel spells a move as a rename to a path under some other parent, so the
/// two are one write here — and the new parent's path is what the answer is
/// built on.
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

/// And the way back up, which is the case that says `parentId` is *sent* rather
/// than left out when there is no parent: a patch that omitted it would leave
/// the folder where it was and answer with a path nothing is at.
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

/// The new name is a mailbox name and the answer is a Camel path, the same two
/// different things a create maps between.
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

/// RFC 8621 §2 makes a name unique among siblings, and a rename is the other
/// way to break that. The folder stays where it was.
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

/// A folder cannot be moved inside itself: the server refuses it, and what it
/// would otherwise make is a subtree with no way back to the account.
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

/// And a folder another client removed while this one still listed it, which
/// gets the variant every other write to a mailbox gives it.
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
