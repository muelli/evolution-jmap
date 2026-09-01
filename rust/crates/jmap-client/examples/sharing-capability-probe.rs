// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Live probe of Track E Phase C's step 1: what does a real Stalwart
// deployment actually advertise and accept for `shareWith` / `myRights` /
// `ShareNotification` (RFC 9670 §4's "Framework for Shared Data")? The
// design in docs/PRINCIPALS-DESIGN.md models the types against the specs;
// this probe checks them against the wire before the mock is extended to
// match and any wiring is TDD'd.
//
// Deliberately uses `Client::single_call` for every `shareWith` write so a
// field-name mismatch shows up as a raw JSON diff rather than a silently
// swallowed serde default.
//
// Usage (needs two distinct accounts already provisioned on the same
// server, e.g. via the server's own admin tooling; set
// JMAP_LIVE_SERVER_REBASE_URLS=1 if reaching the server through an address
// it does not itself advertise):
//   cargo run -p evolution-jmap-client --example sharing-capability-probe -- \
//       <origin> <alice-user> <alice-password> <bob-user> <bob-password>

use jmap_client::{Client, Credentials};
use jmap_proto::contacts::{AddressBook, AddressBookRights};
use jmap_proto::methods::{GetRequest, SetRequest};
use jmap_proto::principals::PrincipalQueryFilter;
use jmap_proto::session::{
    CAPABILITY_CONTACTS, CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_PRINCIPALS,
    CAPABILITY_PRINCIPALS_OWNER,
};
use serde_json::json;
use std::collections::BTreeMap;

fn print_raw(label: &str, value: &serde_json::Value) {
    println!("{label}:\n{}", serde_json::to_string_pretty(value).unwrap());
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(origin), Some(alice_user), Some(alice_pass), Some(bob_user), Some(bob_pass)) = (
        args.next(),
        args.next(),
        args.next(),
        args.next(),
        args.next(),
    ) else {
        eprintln!(
            "usage: sharing-capability-probe <origin> <alice-user> <alice-password> <bob-user> <bob-password>"
        );
        std::process::exit(2);
    };

    let alice = Client::connect(&origin, Credentials::basic(alice_user.clone(), alice_pass))
        .expect("connect as alice");
    let bob = Client::connect(&origin, Credentials::basic(bob_user.clone(), bob_pass))
        .expect("connect as bob");

    println!(
        "server capabilities: {}",
        alice
            .session()
            .capabilities
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "principals:owner advertised: {}",
        alice
            .session()
            .capabilities
            .contains_key(CAPABILITY_PRINCIPALS_OWNER)
    );

    let alice_account = alice
        .primary_account(CAPABILITY_CONTACTS)
        .expect("alice's account");
    let bob_account = bob
        .primary_account(CAPABILITY_CONTACTS)
        .expect("bob's account");
    println!("alice_account = {alice_account}, bob_account = {bob_account}");

    if !alice
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        println!("NOT SUPPORTED: server does not advertise {CAPABILITY_PRINCIPALS} — stopping");
        return;
    }

    // Resolve each other's principal id (RFC 9670 Principal/query by email).
    let bob_principal_id = alice
        .principal_query(&alice_account, PrincipalQueryFilter::email(&bob_user))
        .expect("Principal/query for bob from alice's session")
        .into_iter()
        .next();
    println!("alice resolves bob's principal id: {bob_principal_id:?}");
    let Some(bob_principal_id) = bob_principal_id else {
        println!("NOT FOUND: alice cannot resolve bob's principal — stopping");
        return;
    };

    // Raw Principal/get, to see the real field spelling of the capability bag.
    let principal_get = alice
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_PRINCIPALS],
            "Principal/get",
            &GetRequest {
                account_id: alice_account.clone(),
                ids: Some(vec![bob_principal_id.clone()]),
                ids_ref: None,
                properties: None,
            },
        )
        .expect("Principal/get");
    print_raw("Principal/get(bob) raw response", &principal_get);

    // --- AddressBook sharing (RFC 9610 §2: myRights AND shareWith both standard) ---
    let book = alice
        .address_book_create(
            &alice_account,
            &AddressBook {
                name: "agent-sharing-probe book".into(),
                ..Default::default()
            },
        )
        .expect("AddressBook/set create");
    let book_id = book.id.clone().expect("server-set address book id");
    println!(
        "created address book {book_id}, default myRights = {:?}",
        book.my_rights
    );

    let mut share_with = BTreeMap::new();
    share_with.insert(
        bob_principal_id.clone(),
        AddressBookRights {
            may_read: Some(true),
            may_write: Some(false),
            may_share: Some(false),
            may_delete: Some(false),
            extra: BTreeMap::new(),
        },
    );
    let patch = json!({ "shareWith": share_with });
    let share_response = alice
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_CONTACTS],
            "AddressBook/set",
            &SetRequest::<AddressBook>::new(alice_account.clone()).update(book_id.clone(), patch),
        )
        .expect("AddressBook/set update (shareWith)");
    print_raw("AddressBook/set shareWith raw response", &share_response);

    let books_after = alice
        .single_call(
            &[CAPABILITY_CORE, CAPABILITY_CONTACTS],
            "AddressBook/get",
            &GetRequest::all(alice_account.clone()),
        )
        .expect("AddressBook/get (alice, after share)");
    print_raw(
        "AddressBook/get (alice, after share) raw response",
        &books_after,
    );

    // Can bob now see the shared address book by querying alice's account
    // directly with his own credentials?
    let books_from_bob = bob.single_call(
        &[CAPABILITY_CORE, CAPABILITY_CONTACTS],
        "AddressBook/get",
        &GetRequest::all(alice_account.clone()),
    );
    match books_from_bob {
        Ok(value) => print_raw(
            "AddressBook/get (bob, on alice's account) raw response",
            &value,
        ),
        Err(error) => println!("AddressBook/get (bob, on alice's account) FAILED: {error}"),
    }

    // Does bob see a ShareNotification about this?
    if bob
        .session()
        .capabilities
        .contains_key(CAPABILITY_PRINCIPALS)
    {
        let notifications = bob.single_call(
            &[CAPABILITY_CORE, CAPABILITY_PRINCIPALS],
            "ShareNotification/get",
            &GetRequest::all(bob_account.clone()),
        );
        match notifications {
            Ok(value) => print_raw("ShareNotification/get (bob) raw response", &value),
            Err(error) => println!("ShareNotification/get (bob) FAILED: {error}"),
        }
    }

    // --- Mailbox sharing: RFC 8621 has no standard shareWith, but this
    // server advertises the Stalwart-local `urn:ietf:params:jmap:mail:share`
    // account capability — check whether Mailbox/set actually accepts it.
    let mailboxes = alice.mailbox_get(&alice_account).expect("Mailbox/get");
    if let Some(mailbox) = mailboxes.list.into_iter().next() {
        let mailbox_id = mailbox.id.clone().expect("server-set mailbox id");
        let mut mailbox_share = BTreeMap::new();
        mailbox_share.insert(bob_principal_id.clone(), json!({"mayReadItems": true}));
        let patch = json!({ "shareWith": mailbox_share });
        let mailbox_share_response = alice.single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "Mailbox/set",
            &json!({ "accountId": alice_account, "update": { mailbox_id.to_string(): patch } }),
        );
        match mailbox_share_response {
            Ok(value) => print_raw("Mailbox/set shareWith raw response", &value),
            Err(error) => println!("Mailbox/set shareWith FAILED: {error}"),
        }

        let mailboxes_from_bob = bob.single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "Mailbox/get",
            &GetRequest::all(alice_account.clone()),
        );
        match mailboxes_from_bob {
            Ok(value) => print_raw("Mailbox/get (bob, on alice's account) raw response", &value),
            Err(error) => println!("Mailbox/get (bob, on alice's account) FAILED: {error}"),
        }

        // Cleanup: revert the share so alice's real mailbox is not left
        // shared with bob past this probe run (RFC 8620 §5.3 PatchObject
        // path removal: `shareWith/<principalId>` set to `null`).
        let revert = alice.single_call(
            &[CAPABILITY_CORE, CAPABILITY_MAIL],
            "Mailbox/set",
            &json!({
                "accountId": alice_account,
                "update": { mailbox_id.to_string(): { format!("shareWith/{bob_principal_id}"): null } },
            }),
        );
        match revert {
            Ok(_) => println!("reverted mailbox shareWith"),
            Err(error) => println!("reverting mailbox shareWith FAILED: {error}"),
        }
    } else {
        println!("alice has no mailboxes to probe Mailbox sharing with");
    }

    // Cleanup.
    alice
        .address_book_destroy(&alice_account, &book_id)
        .expect("AddressBook/set destroy (cleanup)");
    println!("cleaned up probe address book {book_id}");
}
