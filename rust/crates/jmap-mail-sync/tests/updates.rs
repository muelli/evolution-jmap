// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Noticing that a mailbox moved: what `Email/changes` is good for, and the
//! question it does not answer.
//!
//! `Mailbox/changes` reports on the account's folders and
//! [`MailSync::folder_tree_since`] can act on it directly. `Email/changes`
//! reports on the account's *messages*, and a folder is not asking about those:
//! it is asking about one mailbox. JMAP files a message by changing its
//! `mailboxIds`, which is an ordinary update to the message — so a delta naming
//! it says only that *something* about it changed, never whether that something
//! moved it in or out of the mailbox being refreshed.
//!
//! So the tests here are about a delta that re-checks membership rather than
//! inferring it: every message the account reports as touched is looked up, and
//! what comes back is either a row for this mailbox or a uid that is not in it
//! any more. Which of the two a message is has to hold for a message another
//! client moved *in*, moved *out*, destroyed, or merely re-flagged — those four
//! are the whole surface, and they are one query apart from each other on the
//! wire.

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{
    Filing, KeywordChange, Keywords, MailSync, MessageFlags, MessageSummary, MessageUpdate,
};
use jmap_mock::{EmailSeed, MockServer, MockServerBuilder};
use jmap_proto::mail::role;
use jmap_proto::{Id, State};

struct Fixture {
    server: MockServer,
    account_id: Id,
}

impl Fixture {
    fn start() -> Self {
        Self::started_with(MockServer::builder())
    }

    fn started_with(builder: MockServerBuilder) -> Self {
        let server = builder.start();
        let account_id = server.account_id();
        Self { server, account_id }
    }

    fn sync(&self) -> MailSync {
        let client = Client::connect(self.server.origin(), Credentials::none()).unwrap();
        MailSync::new(client, self.account_id.clone())
    }

    /// Mutate the account the way another client would — as a state
    /// transition, which is what `Email/changes` reports.
    fn edit<R>(&self, edit: impl FnOnce(&mut jmap_mock::AccountState) -> R) -> R {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        edit(state.account_mut(&self.account_id).unwrap())
    }

    /// A message in `mailbox`, seeded rather than delivered: it predates every
    /// state the tests here ask from, which is what makes it the *old* mail a
    /// delta must not mention.
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
}

/// The delta, for a test that expects the account to have moved.
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

/// A listing has to come with the state it can be brought forward from, or it
/// is a listing that can only ever be taken again in full.
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

/// The answer nearly every refresh gets, and the reason a delta is worth asking
/// for at all: one round trip that says the folder is already right.
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

/// New mail, which is the case the whole mechanism exists for.
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

/// The delta is the account's; the answer is one mailbox's. A message delivered
/// somewhere else is named by `Email/changes` all the same, and a folder that
/// took the delta at its word would show mail that is not in it.
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

/// The move the previous increment learned to *write*, seen from the other
/// side: another client filed a message elsewhere, and the row has to go.
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

/// And the same event from the destination's point of view. The message is not
/// new — nothing about it was created — so a delta that only believed `created`
/// would never show it.
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

/// A message that is gone is gone from every mailbox, and `Email/get` has
/// nothing to check membership against — so `destroyed` is the one part of the
/// delta that is taken at its word.
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

/// The most ordinary change of all — another client, or the same account on a
/// phone, marking a message read. The row is still this mailbox's, and it comes
/// back with the keywords the server now holds.
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

/// The state a delta carries is the one the *next* delta is asked from, and
/// asking with it immediately has to be quiet: a folder that kept reporting the
/// same change would redraw the user's message list on every poll.
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

/// A state the server cannot calculate from — too old, or from some other
/// server entirely — is answered with the mailbox rather than reported. The
/// judgement [`MailSync::folder_tree_since`] makes about the same condition,
/// for the same reason: a Camel folder has nowhere to report it to, and one
/// that failed here would be a folder that never recovers.
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

/// The rows a delta produces are ordered like the rows a listing produces —
/// oldest first, by the server's own clock — because they are appended to the
/// same summary and Camel numbers messages in the order they are added.
#[test]
fn the_rows_a_delta_produces_are_oldest_first() {
    let fixture = Fixture::start();
    let inbox = fixture.edit(|account| account.seed_mailbox("Inbox", Some(role::INBOX)));
    let sync = fixture.sync();
    let (state, held) = sync.messages(&inbox).unwrap();

    // Delivered newest first, so that the order they were created in — which is
    // also the order their ids sort in — is the reverse of the right answer.
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

/// A delta may name more messages than one `Email/get` is allowed to ask about
/// — a mailbox that was moved wholesale is exactly that — and asking for more
/// than the server's `maxObjectsInGet` fails the whole call rather than
/// answering short.
///
/// The mailbox is seeded first so that the delta stays worth following: a folder
/// holding fewer rows than the delta names would list it again instead, which is
/// [`a_delta_costlier_than_the_mailbox_lists_it_again`] and not this.
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

/// The bound on catching up. A delta is asked from a state, and a folder that
/// has been closed for a week is asked from a week-old one: what comes back is
/// every message the *account* touched since — most of them in mailboxes this
/// folder is not, each one of them still an id this call has to look up before
/// it can say so. Listing the one mailbox is the cheaper answer well before
/// that, so a delta that names more messages than the folder holds rows is not
/// followed.
///
/// Asserted on the wire rather than on the answer, because a delta and a listing
/// leave the caller holding the same rows: what differs is only what was asked.
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

    // Five messages the account touched and this mailbox does not hold: the
    // delta would look every one of them up to conclude the inbox is unchanged.
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

/// The other side of the same bound, and the case that matters far more often: a
/// mailbox big enough that catching up is the cheap answer stays caught up, one
/// `Email/get` of the messages that moved, without the whole mailbox being
/// fetched again.
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

/// The floor under the bound. A listing is never one round trip — it is the
/// state, the query, and a `/get` per page — so a delta that fits in a single
/// `Email/get` is cheaper than relisting whatever the mailbox holds, including
/// the nearly empty mailbox that would otherwise relist on every message the
/// account touches anywhere.
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

/// What the bound counts is what has to be looked up. A destroyed message is
/// taken at the delta's word — it is gone from every mailbox and there is
/// nothing left to fetch — so a hundred of them cost the same one round trip as
/// none, and a mailbox that emptied elsewhere must not relist for them.
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

/// A complex delta with a combination of delivered messages, flag/keyword updates,
/// moved messages (in/out), and destroyed messages across multiple mailboxes.
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

    // 1. Deliver a new email to inbox
    let delivered =
        fixture.edit(|account| account.deliver_email(Fixture::message(&inbox, "New Arrival", 12)));

    // 2. Mark flagged message as seen + flagged
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

    // 3. Move moved_out to archive
    fixture
        .sync()
        .file_message(&moved_out, &Filing::moved(inbox.clone(), archive.clone()))
        .unwrap();

    // 4. Destroy doomed message
    assert!(fixture.edit(|account| account.destroy_email(&doomed)));

    // 5. Query delta
    let (_, present, absent) = changed(sync.messages_since(&inbox, &state, held.len()).unwrap());

    // Verify present contains new delivery and updated flagged message
    let present_uids = uids(&present);
    assert!(present_uids.contains(&&delivered));
    assert!(present_uids.contains(&&flagged));
    assert!(!present_uids.contains(&&kept));

    let flagged_row = present.iter().find(|m| m.uid == flagged).unwrap();
    assert!(flagged_row.flags.seen);
    assert!(flagged_row.flags.flagged);

    // Verify absent contains moved_out and doomed
    assert!(absent.contains(&moved_out));
    assert!(absent.contains(&doomed));
    assert!(!absent.contains(&kept));
}
