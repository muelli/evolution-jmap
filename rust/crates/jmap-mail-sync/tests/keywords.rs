// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The write direction: a flag change by the user, sent as an `Email/set`.
//!
//! The write does not send the full row. It sends the *difference* between
//! the keywords of the last listing and the keywords of the row now. Thus the
//! client never removes a keyword that it does not know.

use std::collections::BTreeMap;

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{KeywordChange, Keywords, MailSync, MessageFlags, SyncError};
use jmap_mock::{EmailSeed, MockServer};
use jmap_proto::Id;
use serde_json::json;

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

    /// One message in one mailbox, with the given keywords.
    fn seed_message(&self, keywords: &[&str]) -> Id {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        let account = state.account_mut(&self.account_id).unwrap();
        let mailbox = account.seed_mailbox("Inbox", Some("inbox"));
        let mut seed = EmailSeed::new(
            mailbox,
            ("Bob", "bob@example.com"),
            "Lunch?",
            "One o'clock.",
            "2026-01-15T09:30:00Z",
        );
        for keyword in keywords {
            seed = seed.keyword(keyword);
        }
        account.seed_email(seed)
    }

    /// The current keywords of the message on the server.
    fn keywords_on_server(&self, uid: &Id) -> BTreeMap<String, bool> {
        let state = self.server.state();
        let state = state.lock().unwrap();
        let account = state.account(&self.account_id).unwrap();
        account
            .emails
            .get(uid)
            .expect("the seeded message")
            .keywords
            .clone()
            .unwrap_or_default()
    }
}

/// The flags word of a row that is read and flagged.
fn read_and_flagged() -> MessageFlags {
    MessageFlags {
        seen: true,
        flagged: true,
        ..MessageFlags::default()
    }
}

#[test]
fn the_keywords_of_a_row_are_its_flags_and_its_labels() {
    let keywords = Keywords::new(&read_and_flagged(), &["Work".to_owned()]);

    let names: Vec<&str> = keywords.iter().collect();
    assert_eq!(names, vec!["$flagged", "$seen", "Work"]);
}

#[test]
fn an_attachment_is_a_property_of_a_message_not_a_label_on_it() {
    // The only bit of `MessageFlags` that is not a keyword. As a keyword, it
    // puts a `$hasattachment` label on the server, and all clients show it.
    let flags = MessageFlags {
        attachments: true,
        ..MessageFlags::default()
    };

    assert_eq!(Keywords::new(&flags, &[]).iter().count(), 0);
}

#[test]
fn a_change_names_only_what_changed() {
    let before = Keywords::new(&read_and_flagged(), &["Work".to_owned()]);
    let after = Keywords::new(
        &MessageFlags {
            seen: true,
            ..MessageFlags::default()
        },
        &["Work".to_owned(), "Later".to_owned()],
    );

    let change = KeywordChange::between(&before, &after);

    assert!(!change.is_empty());
    // `$seen` and `Work` are in both sets, so the patch does not include them.
    // A second write of them can overwrite a change by a different client.
    assert_eq!(
        change.patch(),
        json!({"keywords/Later": true, "keywords/$flagged": null})
    );
}

#[test]
fn a_change_that_changes_nothing_is_empty() {
    let keywords = Keywords::new(&read_and_flagged(), &["Work".to_owned()]);

    assert!(KeywordChange::between(&keywords, &keywords).is_empty());
}

#[test]
fn keywords_are_compared_without_regard_to_case() {
    // RFC 8621 §4.1.1 refers to RFC 5788, and keywords are case-insensitive.
    // `Work` on the server and `work` in a row are the same keyword. Else, each
    // synchronisation writes the keyword again.
    let before = Keywords::new(&MessageFlags::default(), &["Work".to_owned()]);
    let after = Keywords::new(&MessageFlags::default(), &["work".to_owned()]);

    assert!(KeywordChange::between(&before, &after).is_empty());
}

#[test]
fn a_keyword_is_removed_under_the_name_the_server_gave_it() {
    // The removal must name the key that the server has. Thus it uses the
    // *previous* spelling, not a normalised one.
    let before = Keywords::new(&MessageFlags::default(), &["Work".to_owned()]);
    let after = Keywords::new(&MessageFlags::default(), &[]);

    assert_eq!(
        KeywordChange::between(&before, &after).patch(),
        json!({"keywords/Work": null})
    );
}

#[test]
fn a_keyword_set_survives_being_written_out_as_the_names_it_holds() {
    // A row on disk keeps only the keyword names. The keywords of the last
    // listing must persist, because the next diff needs a *before* set. Thus
    // the names must give the same set again.
    let keywords = Keywords::new(&read_and_flagged(), &["Work".to_owned()]);

    let restored: Keywords = keywords.iter().map(str::to_owned).collect();

    assert_eq!(restored, keywords);
    assert_eq!(restored.len(), 3);
}

#[test]
fn two_spellings_of_one_keyword_collected_together_are_one_keyword() {
    // The same case folding as [`Keywords::new`]: a set read from disk is a
    // set, not the stored list.
    let restored: Keywords = ["Work".to_owned(), "work".to_owned()].into_iter().collect();

    assert_eq!(restored.len(), 1);
    assert_eq!(restored.iter().collect::<Vec<&str>>(), vec!["Work"]);
}

/// A change applied to a set, as a past event. The folder needs this when the
/// *server* changed and the user also changed the row.
#[test]
fn a_set_takes_the_change_that_was_made_to_it() {
    let before = Keywords::new(&MessageFlags::default(), &["Work".to_owned()]);
    let after = Keywords::new(&read_and_flagged(), &[]);
    let change = KeywordChange::between(&before, &after);

    assert_eq!(before.patched(&change), after);
}

#[test]
fn a_change_applied_to_a_set_it_was_not_taken_from_touches_only_what_it_names() {
    // The real case: the pending change of the user, applied to a new listing.
    // The listing keeps all keywords that the change does not name, also
    // unknown keywords.
    let change = KeywordChange::between(
        &Keywords::new(&MessageFlags::default(), &[]),
        &Keywords::new(&read_and_flagged(), &[]),
    );
    let listed = Keywords::new(&MessageFlags::default(), &["Urgent".to_owned()]);

    let patched = listed.patched(&change);

    assert_eq!(
        patched.iter().collect::<Vec<&str>>(),
        vec!["$flagged", "$seen", "Urgent"]
    );
}

#[test]
fn a_change_removes_a_keyword_however_the_set_spells_it() {
    // The removal uses the spelling of the *previous* set, which can differ
    // from the patched set. The case-folded comparison matches them.
    let change = KeywordChange::between(
        &Keywords::new(&MessageFlags::default(), &["Work".to_owned()]),
        &Keywords::default(),
    );
    let listed = Keywords::new(&MessageFlags::default(), &["WORK".to_owned()]);

    assert!(listed.patched(&change).is_empty());
}

/// The set as the two Camel columns: the flags word and the labels. This is
/// the exact reverse of [`Keywords::new`]. Else, Evolution shows a bit as a
/// label such as `$seen`.
#[test]
fn a_set_splits_back_into_the_flags_and_the_labels_it_was_built_from() {
    let flags = read_and_flagged();
    let tags = vec!["Work".to_owned(), "home/todo".to_owned()];

    let (split_flags, split_tags) = Keywords::new(&flags, &tags).split();

    assert_eq!(split_flags, flags);
    // In case-folded order, which is the order of the set. The row also keeps
    // them as a set.
    assert_eq!(split_tags, vec!["home/todo".to_owned(), "Work".to_owned()]);
}

/// All keyword bits of the word, not only the two usual ones. A bit that comes
/// back as a label shows a reply or spam as a usual message with the label
/// `$answered` or `$junk`. The next write then clears the bit.
#[test]
fn every_keyword_flag_splits_back_into_its_bit_and_never_into_a_label() {
    let flags = MessageFlags {
        seen: true,
        answered: true,
        flagged: true,
        draft: true,
        forwarded: true,
        junk: true,
        not_junk: true,
        attachments: false,
    };

    let (split_flags, split_tags) = Keywords::new(&flags, &[]).split();

    assert_eq!(split_flags, flags);
    assert!(split_tags.is_empty(), "labels: {split_tags:?}");
}

#[test]
fn a_set_holding_a_keyword_is_not_empty() {
    let keywords = Keywords::new(&read_and_flagged(), &[]);

    assert!(!keywords.is_empty());
    assert!(Keywords::default().is_empty());
}

#[test]
fn splitting_a_set_reads_a_keyword_the_server_shouted_as_the_flag_it_is() {
    // Match the case-folded name. Else, `$Seen` from a server gives an unread
    // message with a label.
    let keywords: Keywords = ["$Seen".to_owned()].into_iter().collect();

    let (flags, tags) = keywords.split();

    assert!(flags.seen);
    assert!(tags.is_empty());
}

/// `attachments` is the only field of the word that is not a keyword, so a
/// split never sets it. The server calculates `hasAttachment` (RFC 8621
/// §4.1.1), so the bit comes from the listing, not from the keywords.
#[test]
fn splitting_a_set_never_claims_an_attachment() {
    let keywords = Keywords::new(
        &MessageFlags {
            attachments: true,
            seen: true,
            ..MessageFlags::default()
        },
        &[],
    );

    assert!(!keywords.split().0.attachments);
}

#[test]
fn a_label_with_a_slash_in_it_is_one_keyword_and_not_a_path() {
    // A patch key is a JSON pointer (RFC 8620 §5.3, RFC 6901). An IMAP keyword
    // is an atom, which permits `/` and `~`. Without escapes, `home/todo`
    // patches a `todo` member of a `home` object that does not exist.
    let after = Keywords::new(&MessageFlags::default(), &["home/todo~1".to_owned()]);

    assert_eq!(
        KeywordChange::between(&Keywords::default(), &after).patch(),
        json!({"keywords/home~1todo~01": true})
    );
}

#[test]
fn marking_a_message_read_sets_the_keyword_on_the_server() {
    let fixture = Fixture::start();
    let uid = fixture.seed_message(&[]);

    let change = KeywordChange::between(
        &Keywords::default(),
        &Keywords::new(
            &MessageFlags {
                seen: true,
                ..MessageFlags::default()
            },
            &[],
        ),
    );
    fixture.sync().set_keywords(&uid, &change).unwrap();

    assert_eq!(
        fixture.keywords_on_server(&uid),
        BTreeMap::from([("$seen".to_owned(), true)])
    );
}

#[test]
fn a_keyword_this_client_never_saw_survives_the_change() {
    let fixture = Fixture::start();
    // A different client added `Urgent` after the last listing. The diff does
    // not name it, so the write must not change it.
    let uid = fixture.seed_message(&["$seen", "Urgent"]);

    let change = KeywordChange::between(
        &Keywords::new(
            &MessageFlags {
                seen: true,
                ..MessageFlags::default()
            },
            &[],
        ),
        &Keywords::default(),
    );
    fixture.sync().set_keywords(&uid, &change).unwrap();

    assert_eq!(
        fixture.keywords_on_server(&uid),
        BTreeMap::from([("Urgent".to_owned(), true)])
    );
}

#[test]
fn a_label_with_a_slash_survives_a_round_trip_through_the_server() {
    let fixture = Fixture::start();
    let uid = fixture.seed_message(&[]);

    let change = KeywordChange::between(
        &Keywords::default(),
        &Keywords::new(&MessageFlags::default(), &["home/todo".to_owned()]),
    );
    fixture.sync().set_keywords(&uid, &change).unwrap();

    assert_eq!(
        fixture.keywords_on_server(&uid),
        BTreeMap::from([("home/todo".to_owned(), true)])
    );
}

#[test]
fn a_change_with_nothing_in_it_is_not_a_request() {
    let fixture = Fixture::start();

    // The account does not have this id, so a request gets `notFound`. Success
    // proves that no request was sent. Camel marks rows dirty for other
    // reasons, and these must not cause requests.
    fixture
        .sync()
        .set_keywords(&Id::new("E404"), &KeywordChange::default())
        .expect("a change that says nothing needs no server");
}

#[test]
fn a_message_another_client_deleted_is_reported_as_gone() {
    let fixture = Fixture::start();
    fixture.seed_message(&[]);

    let change = KeywordChange::between(
        &Keywords::default(),
        &Keywords::new(
            &MessageFlags {
                seen: true,
                ..MessageFlags::default()
            },
            &[],
        ),
    );
    let error = fixture
        .sync()
        .set_keywords(&Id::new("E404"), &change)
        .expect_err("a message that is not there cannot be flagged");

    // The same rule as `message_source`: a summary uid comes from the last
    // listing. A missing message is normal, not a broken account.
    match error {
        SyncError::NoSuchMessage(uid) => assert_eq!(uid.as_str(), "E404"),
        other => panic!("expected the message to be reported as gone, got {other:?}"),
    }
}
