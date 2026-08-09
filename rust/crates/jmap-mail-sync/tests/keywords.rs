// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The other direction: a flag the user changed, as the `Email/set` that makes
//! the server agree.
//!
//! Everything `jmap-mail-sync` has done so far reads. This is the first thing
//! it writes, and what is written is deliberately not "the row as it now is" —
//! it is the *difference* between the keywords the last listing found and the
//! keywords the row claims now, so a keyword nobody here knows about is one
//! nobody here removes.

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

    /// One message in one mailbox, carrying the keywords given.
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

    /// What the server holds for the message now.
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

/// The flags word of a row that is read and important.
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
    // The one bit of `MessageFlags` that does not come from a keyword. Sending
    // it as one would put a `$hasattachment` label on the message on the
    // server, and every other client would show it.
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
    // `$seen` and `Work` are in both and appear in neither half of the patch:
    // a patch that re-set them would be a write nobody asked for, over a
    // keyword another client may have just changed.
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
    // RFC 8621 §4.1.1 hands the keyword vocabulary to RFC 5788, whose keywords
    // are case-insensitive. A server that spells a label `Work` and a row that
    // spells it `work` are saying the same thing, and a diff that missed that
    // would rewrite the same keyword on every synchronisation.
    let before = Keywords::new(&MessageFlags::default(), &["Work".to_owned()]);
    let after = Keywords::new(&MessageFlags::default(), &["work".to_owned()]);

    assert!(KeywordChange::between(&before, &after).is_empty());
}

#[test]
fn a_keyword_is_removed_under_the_name_the_server_gave_it() {
    // The other side of the same coin: what has to be taken off the object is
    // the key the server actually has, so removal quotes the *previous*
    // spelling rather than a normalised one.
    let before = Keywords::new(&MessageFlags::default(), &["Work".to_owned()]);
    let after = Keywords::new(&MessageFlags::default(), &[]);

    assert_eq!(
        KeywordChange::between(&before, &after).patch(),
        json!({"keywords/Work": null})
    );
}

#[test]
fn a_keyword_set_survives_being_written_out_as_the_names_it_holds() {
    // What a row kept on disk amounts to. The keywords the last listing found
    // have to outlive the process — a folder that forgot them would have no
    // *before* to diff the next flag change against — and the only thing there
    // is to write down is the names, so collecting them back has to produce
    // the set they came from.
    let keywords = Keywords::new(&read_and_flagged(), &["Work".to_owned()]);

    let restored: Keywords = keywords.iter().map(str::to_owned).collect();

    assert_eq!(restored, keywords);
    assert_eq!(restored.len(), 3);
}

#[test]
fn two_spellings_of_one_keyword_collected_together_are_one_keyword() {
    // The same folding [`Keywords::new`] applies, on the way back in: a set
    // read off disk is a set, not the list it was stored as.
    let restored: Keywords = ["Work".to_owned(), "work".to_owned()].into_iter().collect();

    assert_eq!(restored.len(), 1);
    assert_eq!(restored.iter().collect::<Vec<&str>>(), vec!["Work"]);
}

/// A change read as something that has already happened rather than something
/// to make happen. It is the same value either way — what a keyword set becomes
/// when a change is applied to it is what the folder needs to know when the
/// *server* is the one that changed, and the row it is applying it to is one the
/// user has already touched.
#[test]
fn a_set_takes_the_change_that_was_made_to_it() {
    let before = Keywords::new(&MessageFlags::default(), &["Work".to_owned()]);
    let after = Keywords::new(&read_and_flagged(), &[]);
    let change = KeywordChange::between(&before, &after);

    assert_eq!(before.patched(&change), after);
}

#[test]
fn a_change_applied_to_a_set_it_was_not_taken_from_touches_only_what_it_names() {
    // The case the folder is actually in: the user's outstanding change, whose
    // two ends are the row's own, replayed on top of a fresh listing. Everything
    // the change does not name is the listing's answer and stays exactly as it
    // is — including a keyword this side has never seen.
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
    // The removal names the keyword as the *previous* set spelled it, which is
    // not necessarily how the set being patched spells it — folded comparison is
    // what keeps the two the same keyword.
    let change = KeywordChange::between(
        &Keywords::new(&MessageFlags::default(), &["Work".to_owned()]),
        &Keywords::default(),
    );
    let listed = Keywords::new(&MessageFlags::default(), &["WORK".to_owned()]);

    assert!(listed.patched(&change).is_empty());
}

/// The set as Camel's two columns: the flags word's fields and the labels beside
/// them. [`Keywords::new`] run backwards, and it has to be exactly that — a
/// keyword that came in as a bit and went out as a label would show up in
/// Evolution as a label called `$seen`.
#[test]
fn a_set_splits_back_into_the_flags_and_the_labels_it_was_built_from() {
    let flags = read_and_flagged();
    let tags = vec!["Work".to_owned(), "home/todo".to_owned()];

    let (split_flags, split_tags) = Keywords::new(&flags, &tags).split();

    assert_eq!(split_flags, flags);
    // In folded order, which is the order the set holds them in; the row they
    // are written to keeps them as a set too.
    assert_eq!(split_tags, vec!["home/todo".to_owned(), "Work".to_owned()]);
}

#[test]
fn splitting_a_set_reads_a_keyword_the_server_shouted_as_the_flag_it_is() {
    // The folded name is what the match is against, or a server that spells it
    // `$Seen` would leave the message unread and labelled.
    let keywords: Keywords = ["$Seen".to_owned()].into_iter().collect();

    let (flags, tags) = keywords.split();

    assert!(flags.seen);
    assert!(tags.is_empty());
}

/// `attachments` is the one field of the word that is not a keyword, so a set
/// cannot carry it and splitting one never sets it: `hasAttachment` is a
/// property RFC 8621 §4.1.1 has the server compute, and its bit has to come from
/// the listing rather than from the keywords.
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
    // A patch key is a JSON pointer (RFC 8620 §5.3, RFC 6901) and an IMAP
    // keyword is an atom, which permits `/` and `~`. Unescaped, a label like
    // `home/todo` would patch a `todo` member of a `home` object that is not
    // there — inventing structure inside `keywords` instead of setting one.
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
    // What another client did between our last listing and this write. Neither
    // side of the diff mentions it, so nothing may touch it.
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

    // An id the account has never held: a request would come back `notFound`,
    // so succeeding is what proves none was sent. Camel marks a row dirty for
    // reasons that are not keywords, and each of those must cost nothing.
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

    // The same judgement `message_source` makes about the same situation: a
    // uid in a folder summary is a claim about the last listing, and a row
    // whose message is gone is ordinary rather than a broken account.
    match error {
        SyncError::NoSuchMessage(uid) => assert_eq!(uid.as_str(), "E404"),
        other => panic!("expected the message to be reported as gone, got {other:?}"),
    }
}
