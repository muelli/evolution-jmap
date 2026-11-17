// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `isSubscribed` on a shared mailbox is the grantee's own flag, not the
//! owner's. This test checks that a grantee's `Mailbox/set` of its own
//! `isSubscribed` for a shared mailbox leaves the owner's own flag on the
//! same mailbox unchanged.
//!
//! ## Running it
//!
//! See `docs/manual-test-live-server.md`. Set `JMAP_LIVE_SERVER_URL`,
//! `JMAP_LIVE_SERVER_WRITE_USER`/`_PASSWORD` (the owner) and
//! `JMAP_LIVE_SERVER_RECIPIENT_USER`/`_PASSWORD` (the grantee), then:
//!
//! ```console
//! $ cargo test -p evolution-jmap-mail-sync --test live_server_shared_mailbox_subscription -- --ignored
//! ```

use jmap_proto::mail::Mailbox;
use jmap_proto::principals::PrincipalQueryFilter;
use jmap_proto::session::{CAPABILITY_MAIL, CAPABILITY_PRINCIPALS};
use jmap_test_support::{connect_owner, connect_recipient_with_user, unique_suffix};
use serde_json::json;

/// A grantee's own `isSubscribed` write on a shared mailbox must not reach
/// the owner's own flag for the same mailbox: the two are separate per RFC
/// 8621 Section 2, each keyed to the account `Mailbox/set` runs under.
#[test]
#[ignore = "needs a real JMAP server; see docs/manual-test-live-server.md"]
fn a_grantees_is_subscribed_change_leaves_the_owners_own_flag_alone() {
    let Some((owner, _owner_address)) = connect_owner() else {
        eprintln!(
            "JMAP_LIVE_SERVER_WRITE_USER/_PASSWORD not set; skipping the shared subscription test"
        );
        return;
    };
    let Some((recipient, recipient_address)) = connect_recipient_with_user() else {
        eprintln!(
            "JMAP_LIVE_SERVER_RECIPIENT_USER/_PASSWORD not set; skipping the shared subscription test"
        );
        return;
    };
    if !recipient
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        eprintln!(
            "server does not advertise {CAPABILITY_PRINCIPALS}; skipping the shared subscription test"
        );
        return;
    }

    let owner_account_id = owner
        .primary_account(CAPABILITY_MAIL)
        .expect("the write-test account needs the mail capability");

    let recipient_principal_id = owner
        .principal_query(
            &owner_account_id,
            PrincipalQueryFilter::email(&recipient_address),
        )
        .ok()
        .and_then(|ids| ids.into_iter().next())
        .expect("the owner cannot resolve the recipient's principal id by email");

    let suffix = unique_suffix();
    let mailbox = owner
        .mailbox_create(
            &owner_account_id,
            &Mailbox::new(format!("agent-mailsync-shared-sub-{suffix}")),
        )
        .expect("Mailbox/set create failed against the real server");
    let mailbox_id = mailbox
        .id
        .clone()
        .expect("the server named the new mailbox");

    owner
        .mailbox_update(
            &owner_account_id,
            &mailbox_id,
            json!({"shareWith": {recipient_principal_id.as_str(): {"mayReadItems": true}}}),
        )
        .expect("Mailbox/set shareWith failed against the real server");

    let owner_subscribed_before = owner
        .mailbox_get(&owner_account_id)
        .expect("Mailbox/get failed against the real server")
        .list
        .into_iter()
        .find(|candidate| candidate.id.as_ref() == Some(&mailbox_id))
        .expect("the owner's own Mailbox/get should list the mailbox it owns")
        .is_subscribed;

    recipient
        .mailbox_update(
            &owner_account_id,
            &mailbox_id,
            json!({"isSubscribed": true}),
        )
        .expect("the grantee's own Mailbox/set of isSubscribed failed against the real server");

    let owner_subscribed_after = owner
        .mailbox_get(&owner_account_id)
        .expect("Mailbox/get failed against the real server")
        .list
        .into_iter()
        .find(|candidate| candidate.id.as_ref() == Some(&mailbox_id))
        .expect("the owner's own Mailbox/get should still list the mailbox it owns")
        .is_subscribed;

    assert_eq!(
        owner_subscribed_before, owner_subscribed_after,
        "the grantee's own isSubscribed write must not change the owner's own flag"
    );

    owner
        .mailbox_destroy(&owner_account_id, &mailbox_id)
        .expect("Mailbox/set destroy failed against the real server (cleanup)");
}
