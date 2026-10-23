// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Detection of changes in a mailbox: what `Email/changes` can tell, and what
//! it cannot tell.
//!
//! `Mailbox/changes` reports on the folders of the account, and
//! [`MailSync::folder_tree_since`] can use it directly. `Email/changes`
//! reports on the *messages* of the account, but a folder needs data about one
//! mailbox. JMAP files a message with a change to its `mailboxIds`, which is a
//! usual update. Thus a delta tells only that *something* changed, not whether
//! the message moved into or out of the refreshed mailbox.
//!
//! Thus the delta checks membership again and does not infer it. The code
//! looks up each changed message. The result is a row for this mailbox, or a
//! uid that is no longer in it. This must be correct when a different client
//! moves a message *in*, moves it *out*, destroys it, or changes only its
//! flags.

use std::collections::BTreeMap;

use jmap_mail_sync::{
    Filing, KeywordChange, Keywords, MessageFlags, MessageSummary, MessageUpdate, SyncError,
};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::mail::role;
use jmap_proto::{Id, State};

mod common;
use common::Fixture;

impl Fixture {
    /// Seed (not deliver) a message in `mailbox`. It is older than each state
    /// that the tests use, so it is *old* mail that a delta must not name.
    fn seed(&self, mailbox: &Id, subject: &str, hour: u32) -> Id {
        self.edit(|account| account.seed_email(Self::message(mailbox, subject, hour)))
    }

    fn message(mailbox: &Id, subject: &str, hour: u32) -> EmailSeed {
        EmailSeed::new(
            mailbox.clone(),
            ("Bob", "bob@example.com"),
            subject,
            "text",
            &format!("2026-01-15T{hour:02}:00:00Z"),
        )
    }

    fn subjects(messages: &[MessageSummary]) -> Vec<&str> {
        messages
            .iter()
            .map(|message| message.subject.as_deref().unwrap_or_default())
            .collect()
    }

    /// Overwrite the `mailboxIds` of a message in the store, as an update by a
    /// different client. This makes a state that a conforming server never
    /// sends (RFC 8621 §4.6 makes each present value `true`). The store still
    /// records the change for `Email/changes`.
    fn set_mailbox_ids(&self, uid: &Id, mailbox_ids: BTreeMap<Id, bool>) {
        self.edit(|account| {
            account.emails.transaction(|txn| {
                let mut email = txn.get(uid).expect("the seeded message").clone();
                email.mailbox_ids = Some(mailbox_ids);
                txn.update(uid, email);
            })
        });
    }
}

/// The delta, for a test that expects a change in the account.
fn changed(update: MessageUpdate) -> (State, Vec<MessageSummary>, Vec<Id>) {
    match update {
        MessageUpdate::Changed {
            state,
            present,
            absent,
        } => (state, present, absent),
        MessageUpdate::Unchanged(state) => {
            panic!("expected a delta, the server reported nothing new at {state}")
        }
        MessageUpdate::Relisted { state, .. } => {
            panic!("expected a delta, the mailbox was listed again at {state}")
        }
    }
}

fn uids(messages: &[MessageSummary]) -> Vec<&Id> {
    messages.iter().map(|message| &message.uid).collect()
}

/// A listing must include its state. Without a state, the client can only
/// list again in full.
#[test]
fn a_listing_carries_a_state_to_ask_from() {
    let fixture = Fixture::start();
    let inbox = fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    fixture.seed(&inbox, "First", 9);
    let sync = fixture.sync();

    let (state, messages) = sync.messages(&inbox).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(
        state,
        fixture.edit(|account| account.emails.state()),
        "the state is the account's own, not one invented here"
    );
}

/// The usual refresh result, and the reason for a delta: one round trip tells
/// that the folder is already correct.
#[test]
fn a_mailbox_nobody_touched_reports_nothing() {
    let fixture = Fixture::start();
    let inbox = fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    fixture.seed(&inbox, "First", 9);
    let sync = fixture.sync();
    let (state, held) = sync.messages(&inbox).unwrap();

    match sync.messages_since(&inbox, &state, held.len()).unwrap() {
        MessageUpdate::Unchanged(unchanged) => assert_eq!(unchanged, state),
        other => panic!("nothing changed, and the delta was {other:?}"),
    }
}

/// New mail, the main purpose of the delta.
#[test]
fn a_message_that_arrived_comes_back_as_a_row() {
    let fixture = Fixture::start();
    let inbox = fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    fixture.seed(&inbox, "Old", 9);
    let sync = fixture.sync();
    let (state, held) = sync.messages(&inbox).unwrap();

    let arrived =
        fixture.edit(|account| account.deliver_email(Fixture::message(&inbox, "New", 10)));

    let (_, present, absent) = changed(sync.messages_since(&inbox, &state, held.len()).unwrap());
    assert_eq!(Fixture::subjects(&present), ["New"]);
    assert_eq!(uids(&present), [&arrived]);
    assert!(absent.is_empty(), "nothing left the mailbox: {absent:?}");
}

/// The delta is for the account, but the result is for one mailbox.
/// `Email/changes` also names a message delivered to a different mailbox. If
/// the folder trusts the delta, it shows mail that is not in it.
#[test]
fn a_message_that_arrived_elsewhere_is_not_a_row_of_this_mailbox() {
    let fixture = Fixture::start();
    let (inbox, other) = fixture.edit(|account| {
        (
            account.seed_mailbox("Inbox", Some(role::INBOX)),
            account.seed_mailbox("Receipts", None),
        )
    });
    fixture.seed(&inbox, "Old", 9);
    let sync = fixture.sync();
    let (state, held) = sync.messages(&inbox).unwrap();

    fixture.edit(|account| account.deliver_email(Fixture::message(&other, "Receipt", 10)));

    let (_, present, _) = changed(sync.messages_since(&inbox, &state, held.len()).unwrap());
    assert!(
        present.is_empty(),
        "a message of another mailbox was listed as this one's: {:?}",
        Fixture::subjects(&present)
    );
}

/// A move by a different client to a different mailbox. The row must go.
#[test]
fn a_message_moved_out_of_the_mailbox_is_reported_absent() {
    let fixture = Fixture::start();
    let (inbox, archive) = fixture.edit(|account| {
        (
            account.seed_mailbox("Inbox", Some(role::INBOX)),
            account.seed_mailbox("Archive", Some(role::ARCHIVE)),
        )
    });
    let moved = fixture.seed(&inbox, "Filed away", 9);
    fixture.seed(&inbox, "Staying", 10);
    let sync = fixture.sync();
    let (state, held) = sync.messages(&inbox).unwrap();

    fixture
        .sync()
        .file_message(&moved, &Filing::moved(inbox.clone(), archive))
        .unwrap();

    let (_, present, absent) = changed(sync.messages_since(&inbox, &state, held.len()).unwrap());
    assert_eq!(absent, [moved]);
    assert!(
        present.is_empty(),
        "the message that left was listed as still here: {:?}",
        Fixture::subjects(&present)
    );
}

/// A `false` entry in place of a missing key violates RFC 8621 §4.6, as for
/// the input that `message_mailboxes` refuses. As absence, the entry is only a
/// guess, so the code refuses it. It does not report that the message left.
#[test]
fn a_false_valued_mailbox_entry_is_refused_rather_than_guessed_at() {
    let fixture = Fixture::start();
    let inbox = fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    let flagged = fixture.seed(&inbox, "Still here, allegedly not", 9);
    let sync = fixture.sync();
    let (state, held) = sync.messages(&inbox).unwrap();

    fixture.set_mailbox_ids(&flagged, BTreeMap::from([(inbox.clone(), false)]));

    let error = sync
        .messages_since(&inbox, &state, held.len())
        .expect_err("a false-valued mailboxIds entry must not be guessed at");
    match error {
        SyncError::Client(jmap_client::Error::Protocol(message)) => {
            assert!(
                message.contains("false-valued"),
                "expected a message naming the false-valued entry, got: {message}"
            );
        }
        other => panic!("expected a protocol error, got {other:?}"),
    }
}

/// The same move, seen from the destination. The message is not new, so a
/// delta that reads only `created` does not show it.
#[test]
fn a_message_moved_into_the_mailbox_is_reported_as_a_row() {
    let fixture = Fixture::start();
    let (inbox, archive) = fixture.edit(|account| {
        (
            account.seed_mailbox("Inbox", Some(role::INBOX)),
            account.seed_mailbox("Archive", Some(role::ARCHIVE)),
        )
    });
    let moved = fixture.seed(&inbox, "Filed away", 9);
    let sync = fixture.sync();
    let (state, before) = sync.messages(&archive).unwrap();
    assert!(before.is_empty());

    fixture
        .sync()
        .file_message(&moved, &Filing::moved(inbox, archive.clone()))
        .unwrap();

    let (_, present, absent) =
        changed(sync.messages_since(&archive, &state, before.len()).unwrap());
    assert_eq!(uids(&present), [&moved]);
    assert_eq!(
        Fixture::subjects(&present),
        ["Filed away"],
        "a row that arrives by a delta is a whole row, not a bare uid"
    );
    assert!(absent.is_empty(), "nothing left the archive: {absent:?}");
}

/// A destroyed message is gone from all mailboxes, and `Email/get` has no
/// membership to check. Thus `destroyed` is the only part of the delta that
/// the code trusts without a check.
#[test]
fn a_destroyed_message_is_reported_absent() {
    let fixture = Fixture::start();
    let inbox = fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    let doomed = fixture.seed(&inbox, "Deleted elsewhere", 9);
    let sync = fixture.sync();
    let (state, held) = sync.messages(&inbox).unwrap();

    assert!(fixture.edit(|account| account.destroy_email(&doomed)));

    let (_, present, absent) = changed(sync.messages_since(&inbox, &state, held.len()).unwrap());
    assert_eq!(absent, [doomed]);
    assert!(present.is_empty());
}

/// The most usual change: a different client (for example a phone) marks a
/// message as read. The row stays in this mailbox, with the current keywords
/// from the server.
#[test]
fn a_flag_another_client_set_comes_back_on_the_row() {
    let fixture = Fixture::start();
    let inbox = fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    let read = fixture.seed(&inbox, "Read on the phone", 9);
    let sync = fixture.sync();
    let (state, before) = sync.messages(&inbox).unwrap();
    assert!(!before[0].flags.seen);

    let unread = Keywords::new(&MessageFlags::default(), &[]);
    let read_now = Keywords::new(
        &MessageFlags {
            seen: true,
            ..MessageFlags::default()
        },
        &[],
    );
    fixture
        .sync()
        .set_keywords(&read, &KeywordChange::between(&unread, &read_now))
        .unwrap();

    let (_, present, absent) = changed(sync.messages_since(&inbox, &state, before.len()).unwrap());
    assert_eq!(uids(&present), [&read]);
    assert!(present[0].flags.seen, "the row did not come back read");
    assert!(absent.is_empty(), "a re-flagged message did not leave");
}

/// The state of a delta is the start of the *next* delta. A request with it
/// directly after must report no change. Else, each poll draws the message
/// list again.
#[test]
fn the_state_a_delta_carries_is_the_one_to_ask_from_next() {
    let fixture = Fixture::start();
    let inbox = fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    let sync = fixture.sync();
    let (state, held) = sync.messages(&inbox).unwrap();
    fixture.edit(|account| account.deliver_email(Fixture::message(&inbox, "New", 10)));

    let (next, _, _) = changed(sync.messages_since(&inbox, &state, held.len()).unwrap());
    assert_ne!(next, state);

    match sync.messages_since(&inbox, &next, held.len()).unwrap() {
        MessageUpdate::Unchanged(unchanged) => assert_eq!(unchanged, next),
        other => panic!("the same change was reported twice: {other:?}"),
    }
}

/// A state that the server cannot use (too old, or from a different server)
/// causes a new listing, not an error. [`MailSync::folder_tree_since`] does
/// the same: a Camel folder cannot report this error, and a failure here is
/// permanent.
#[test]
fn a_state_the_server_cannot_calculate_from_lists_the_mailbox_again() {
    let fixture = Fixture::start();
    let inbox = fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    fixture.seed(&inbox, "First", 9);
    fixture.seed(&inbox, "Second", 10);
    let sync = fixture.sync();

    match sync
        .messages_since(&inbox, &State::new("nonsense"), 2)
        .unwrap()
    {
        MessageUpdate::Relisted { state, messages } => {
            assert_eq!(Fixture::subjects(&messages), ["First", "Second"]);
            assert_eq!(state, fixture.edit(|account| account.emails.state()));
        }
        other => panic!("a state the server cannot use produced {other:?}"),
    }
}

/// The same unknown state, with the Stalwart answer: `invalidArguments`, not
/// the `cannotCalculateChanges` of RFC 8620 §5.2. A Camel store cannot report
/// it, so the result must also be a new listing.
#[test]
fn a_real_stalwart_shaped_unrecognized_state_lists_the_mailbox_again_too() {
    let fixture =
        Fixture::started_with(MockServer::builder().stalwart_shaped_unrecognized_changes_state());
    let inbox = fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    fixture.seed(&inbox, "First", 9);
    fixture.seed(&inbox, "Second", 10);
    let sync = fixture.sync();

    match sync
        .messages_since(&inbox, &State::new("nonsense"), 2)
        .unwrap()
    {
        MessageUpdate::Relisted { state, messages } => {
            assert_eq!(Fixture::subjects(&messages), ["First", "Second"]);
            assert_eq!(state, fixture.edit(|account| account.emails.state()));
        }
        other => panic!("a state the server cannot use produced {other:?}"),
    }
}

/// Delta rows have the same order as listing rows: oldest first, by server
/// time. They go into the same summary, and Camel numbers messages in the
/// order of addition.
#[test]
fn the_rows_a_delta_produces_are_oldest_first() {
    let fixture = Fixture::start();
    let inbox = fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    let sync = fixture.sync();
    let (state, held) = sync.messages(&inbox).unwrap();

    // Deliver newest first. Creation order (and thus id order) is then the
    // reverse of the correct order.
    for (index, hour) in [12, 11, 10].into_iter().enumerate() {
        fixture.edit(|account| {
            account.deliver_email(Fixture::message(&inbox, &format!("Message {index}"), hour))
        });
    }

    let (_, present, _) = changed(sync.messages_since(&inbox, &state, held.len()).unwrap());
    assert_eq!(
        Fixture::subjects(&present),
        ["Message 2", "Message 1", "Message 0"]
    );
}

/// A delta can name more messages than one `Email/get` permits, for example
/// after a move of a full mailbox. A request for more than `maxObjectsInGet`
/// makes the full call fail.
///
/// Seed the mailbox first, so that the client follows the delta. With fewer
/// rows than the delta names, the folder lists again
/// ([`a_delta_costlier_than_the_mailbox_lists_it_again`]).
#[test]
fn more_changed_messages_than_one_get_may_ask_about_are_fetched_in_several() {
    let fixture = Fixture::started_with(MockServer::builder().objects_in_get(2));
    let inbox = fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    for index in 0..5u32 {
        fixture.seed(&inbox, &format!("Seeded {index}"), index);
    }
    let sync = fixture.sync();
    let (state, held) = sync.messages(&inbox).unwrap();
    assert_eq!(held.len(), 5);

    for index in 0..5u32 {
        fixture.edit(|account| {
            account.deliver_email(Fixture::message(
                &inbox,
                &format!("Message {index}"),
                10 + index,
            ))
        });
    }

    let (_, present, _) = changed(sync.messages_since(&inbox, &state, held.len()).unwrap());
    assert_eq!(present.len(), 5);
    assert_eq!(
        Fixture::subjects(&present),
        [
            "Message 0",
            "Message 1",
            "Message 2",
            "Message 3",
            "Message 4"
        ],
        "chunking must not reorder or drop a row"
    );
}

/// The limit on a delta. For a folder closed for a week, the delta starts at a
/// week-old state. It names all messages that the *account* changed since,
/// mostly in other mailboxes, and the code must look up each id. A listing of
/// the mailbox costs less. Thus the client does not follow a delta that names
/// more messages than the folder has rows.
///
/// The test checks the requests, not the result, because a delta and a listing
/// give the same rows.
#[test]
fn a_delta_costlier_than_the_mailbox_lists_it_again() {
    let fixture = Fixture::started_with(MockServer::builder().objects_in_get(2));
    let (inbox, archive) = fixture.edit(|account| {
        (
            account.seed_mailbox("Inbox", Some(role::INBOX)),
            account.seed_mailbox("Archive", Some(role::ARCHIVE)),
        )
    });
    fixture.seed(&inbox, "First", 9);
    fixture.seed(&inbox, "Second", 10);
    let sync = fixture.sync();
    let (state, held) = sync.messages(&inbox).unwrap();
    assert_eq!(held.len(), 2);

    // Five changed messages that are not in this mailbox. The delta looks up
    // each of them, only to find no change in the inbox.
    for index in 0..5u32 {
        fixture.edit(|account| {
            account.deliver_email(Fixture::message(
                &archive,
                &format!("Filed {index}"),
                11 + index,
            ))
        });
    }

    let asked = fixture.server.method_calls().len();
    match sync.messages_since(&inbox, &state, held.len()).unwrap() {
        MessageUpdate::Relisted { state, messages } => {
            assert_eq!(Fixture::subjects(&messages), ["First", "Second"]);
            assert_eq!(state, fixture.edit(|account| account.emails.state()));
        }
        other => panic!("a delta the mailbox is smaller than produced {other:?}"),
    }

    let calls = fixture.server.method_calls().split_off(asked);
    assert!(
        calls.iter().any(|call| call == "Email/query"),
        "the mailbox was not listed again: {calls:?}"
    );
}

/// The other side of the limit, and the more usual case. For a large mailbox,
/// the delta costs less: `Email/get` of the changed messages only, with no new
/// fetch of the full mailbox.
#[test]
fn a_delta_smaller_than_the_mailbox_is_followed() {
    let fixture = Fixture::started_with(MockServer::builder().objects_in_get(2));
    let (inbox, archive) = fixture.edit(|account| {
        (
            account.seed_mailbox("Inbox", Some(role::INBOX)),
            account.seed_mailbox("Archive", Some(role::ARCHIVE)),
        )
    });
    for index in 0..4u32 {
        fixture.seed(&inbox, &format!("Held {index}"), index);
    }
    let sync = fixture.sync();
    let (state, held) = sync.messages(&inbox).unwrap();
    assert_eq!(held.len(), 4);

    for index in 0..3u32 {
        fixture.edit(|account| {
            account.deliver_email(Fixture::message(
                &archive,
                &format!("Filed {index}"),
                11 + index,
            ))
        });
    }

    let asked = fixture.server.method_calls().len();
    let (_, present, absent) = changed(sync.messages_since(&inbox, &state, held.len()).unwrap());
    assert!(present.is_empty(), "nothing reached the inbox: {present:?}");
    assert_eq!(absent.len(), 3, "the messages filed elsewhere: {absent:?}");

    let calls = fixture.server.method_calls().split_off(asked);
    assert!(
        !calls.iter().any(|call| call == "Email/query"),
        "the mailbox was listed again for a delta it is bigger than: {calls:?}"
    );
}

/// The minimum limit. A listing is more than one round trip: the state, the
/// query, and one `/get` per page. Thus a delta in one `Email/get` costs less
/// than a listing. Else, an almost empty mailbox lists again for each change
/// in the account.
#[test]
fn a_delta_that_fits_in_one_get_is_followed_however_small_the_mailbox() {
    let fixture = Fixture::started_with(MockServer::builder().objects_in_get(2));
    let (inbox, archive) = fixture.edit(|account| {
        (
            account.seed_mailbox("Inbox", Some(role::INBOX)),
            account.seed_mailbox("Archive", Some(role::ARCHIVE)),
        )
    });
    fixture.seed(&inbox, "Only", 9);
    let sync = fixture.sync();
    let (state, held) = sync.messages(&inbox).unwrap();
    assert_eq!(held.len(), 1);

    for index in 0..2u32 {
        fixture.edit(|account| {
            account.deliver_email(Fixture::message(
                &archive,
                &format!("Filed {index}"),
                11 + index,
            ))
        });
    }

    let asked = fixture.server.method_calls().len();
    let (_, _, absent) = changed(sync.messages_since(&inbox, &state, held.len()).unwrap());
    assert_eq!(absent.len(), 2);

    let calls = fixture.server.method_calls().split_off(asked);
    assert!(
        !calls.iter().any(|call| call == "Email/query"),
        "a delta of one `Email/get` listed the mailbox instead: {calls:?}"
    );
}

/// The limit counts only the messages to look up. The code trusts the delta
/// for a destroyed message, because there is nothing to fetch. Thus destroyed
/// messages cost no requests, and do not cause a new listing.
#[test]
fn messages_the_delta_only_reports_gone_do_not_count_toward_the_bound() {
    let fixture = Fixture::started_with(MockServer::builder().objects_in_get(2));
    let (inbox, archive) = fixture.edit(|account| {
        (
            account.seed_mailbox("Inbox", Some(role::INBOX)),
            account.seed_mailbox("Archive", Some(role::ARCHIVE)),
        )
    });
    fixture.seed(&inbox, "Only", 9);
    let doomed: Vec<Id> = (0..5u32)
        .map(|index| fixture.seed(&archive, &format!("Doomed {index}"), 11 + index))
        .collect();
    let sync = fixture.sync();
    let (state, held) = sync.messages(&inbox).unwrap();
    assert_eq!(held.len(), 1);

    for uid in &doomed {
        assert!(fixture.edit(|account| account.destroy_email(uid)));
    }

    let asked = fixture.server.method_calls().len();
    let (_, present, absent) = changed(sync.messages_since(&inbox, &state, held.len()).unwrap());
    assert!(present.is_empty());
    assert_eq!(absent.len(), 5, "the destroyed messages: {absent:?}");

    let calls = fixture.server.method_calls().split_off(asked);
    assert!(
        !calls.iter().any(|call| call == "Email/query"),
        "destroyed messages counted as messages to fetch: {calls:?}"
    );
}

/// A complex delta with delivered messages, keyword updates, a move out, and a
/// destroyed message, across two mailboxes.
#[test]
fn mixed_delta_batches_with_concurrent_additions_removals_and_flag_updates() {
    let fixture = Fixture::start();
    let (inbox, archive) = fixture.edit(|account| {
        (
            account.seed_mailbox("Inbox", Some(role::INBOX)),
            account.seed_mailbox("Archive", Some(role::ARCHIVE)),
        )
    });
    let kept = fixture.seed(&inbox, "Kept Message", 8);
    let flagged = fixture.seed(&inbox, "Flagged Message", 9);
    let moved_out = fixture.seed(&inbox, "Moved Out", 10);
    let doomed = fixture.seed(&inbox, "Doomed Message", 11);
    let sync = fixture.sync();
    let (state, held) = sync.messages(&inbox).unwrap();
    assert_eq!(held.len(), 4);

    // 1. Deliver a new email to the inbox.
    let delivered =
        fixture.edit(|account| account.deliver_email(Fixture::message(&inbox, "New Arrival", 12)));

    // 2. Mark the flagged message as seen and flagged.
    let unread = Keywords::new(&MessageFlags::default(), &[]);
    let seen_flagged = Keywords::new(
        &MessageFlags {
            seen: true,
            flagged: true,
            ..MessageFlags::default()
        },
        &[],
    );
    fixture
        .sync()
        .set_keywords(&flagged, &KeywordChange::between(&unread, &seen_flagged))
        .unwrap();

    // 3. Move `moved_out` to the archive.
    fixture
        .sync()
        .file_message(&moved_out, &Filing::moved(inbox.clone(), archive.clone()))
        .unwrap();

    // 4. Destroy the doomed message.
    assert!(fixture.edit(|account| account.destroy_email(&doomed)));

    // 5. Get the delta.
    let (_, present, absent) = changed(sync.messages_since(&inbox, &state, held.len()).unwrap());

    // `present` contains the new delivery and the updated flagged message.
    let present_uids = uids(&present);
    assert!(present_uids.contains(&&delivered));
    assert!(present_uids.contains(&&flagged));
    assert!(!present_uids.contains(&&kept));

    let flagged_row = present.iter().find(|m| m.uid == flagged).unwrap();
    assert!(flagged_row.flags.seen);
    assert!(flagged_row.flags.flagged);

    // `absent` contains `moved_out` and `doomed`.
    assert!(absent.contains(&moved_out));
    assert!(absent.contains(&doomed));
    assert!(!absent.contains(&kept));
}
