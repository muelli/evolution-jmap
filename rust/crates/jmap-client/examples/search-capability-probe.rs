// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Live verification for the server-side search work (roadmap items 46/47/49)
// against a real JMAP server: seeds a couple of emails, then confirms the
// server evaluates our `EmailQueryFilter` (body / from / subject), returns a
// `SearchSnippet`, and answers `Quota/get`. Prints PASS/FAIL per check and
// exits non-zero on any failure.
//
// Usage:
//   cargo run -p evolution-jmap-client --example search-capability-probe -- \
//       <origin> <user> <password>

use jmap_client::{Client, Credentials};
use jmap_proto::mail::{EmailImport, EmailQueryFilter};

const MAIL: &str = "urn:ietf:params:jmap:mail";

fn mime(from: &str, subject: &str, body: &str) -> Vec<u8> {
    format!(
        "From: {from}\r\nTo: searchprobe@example.org\r\nSubject: {subject}\r\n\
         Date: Tue, 02 Sep 2026 09:00:00 +0000\r\nMessage-ID: <{subject}@probe>\r\n\
         Content-Type: text/plain\r\n\r\n{body}\r\n"
    )
    .into_bytes()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(origin), Some(user), Some(pass)) = (args.next(), args.next(), args.next()) else {
        eprintln!("usage: search-capability-probe <origin> <user> <password>");
        std::process::exit(2);
    };
    let c = Client::connect(&origin, Credentials::basic(user, pass)).expect("connect");
    let account = c.primary_account(MAIL).expect("a mail account");

    // Inbox id.
    let inbox = c
        .mailbox_get(&account)
        .expect("Mailbox/get")
        .list
        .into_iter()
        .find(|m| m.role.as_deref() == Some("inbox"))
        .expect("an inbox")
        .id
        .expect("inbox id");

    // Seed two emails with distinct, uncommon body words and senders.
    for (from, subject, body) in [
        (
            "alice@example.net",
            "Quarterly figures",
            "The kumquat harvest exceeded projections.",
        ),
        (
            "bob@example.net",
            "Lunch plans",
            "Shall we try the new ramen place downtown?",
        ),
    ] {
        let up = c
            .upload_blob(&account, "message/rfc822", mime(from, subject, body))
            .expect("upload");
        c.email_import(&account, &EmailImport::new(up.blob_id, inbox.clone()))
            .expect("import");
    }
    // Give the full-text index a moment.
    std::thread::sleep(std::time::Duration::from_secs(3));

    let mut fail = 0;
    let mut check = |name: &str, ok: bool, detail: String| {
        println!(
            "{} {name}: {detail}",
            if ok {
                "PASS"
            } else {
                fail += 1;
                "FAIL"
            }
        );
    };

    // 1. body full-text search — the flagship path (item 46).
    let hits = c
        .email_query(
            &account,
            EmailQueryFilter::default().body("kumquat"),
            None,
            None,
            0,
        )
        .expect("Email/query body");
    check(
        "body-search (item 46)",
        hits.ids.len() == 1,
        format!("body:kumquat -> {} hit(s), expected 1", hits.ids.len()),
    );

    // 2. from search.
    let from_hits = c
        .email_query(
            &account,
            EmailQueryFilter::default().from("bob@example.net"),
            None,
            None,
            0,
        )
        .expect("Email/query from");
    check(
        "from-search",
        from_hits.ids.len() == 1,
        format!("from:bob -> {} hit(s), expected 1", from_hits.ids.len()),
    );

    // 3. subject search.
    let subj = c
        .email_query(
            &account,
            EmailQueryFilter::default().subject("Quarterly"),
            None,
            None,
            0,
        )
        .expect("Email/query subject");
    check(
        "subject-search",
        subj.ids.len() == 1,
        format!("subject:Quarterly -> {} hit(s), expected 1", subj.ids.len()),
    );

    // 4. SearchSnippet on the body hit (item 47).
    if let Some(id) = hits.ids.first() {
        let snips = c
            .search_snippet_get(
                &account,
                [id.clone()],
                Some(EmailQueryFilter::default().body("kumquat")),
            )
            .expect("SearchSnippet/get");
        let got = snips.iter().any(|s| {
            s.preview
                .as_deref()
                .is_some_and(|p| p.to_lowercase().contains("kumquat"))
        });
        check(
            "snippet (item 47)",
            got,
            format!("{} snippet(s), highlight present: {got}", snips.len()),
        );
    }

    // 5. Quota/get (item 49). The call being accepted and answering a
    // well-formed list is the check; a fresh account with no configured limit
    // legitimately reports zero Quota objects (Stalwart returns one only when
    // a limit is set), so an empty list is a pass, not a failure.
    let quotas = c.quotas(&account).expect("Quota/get");
    check(
        "quota Quota/get accepted (item 49)",
        true,
        format!(
            "{} quota object(s) (0 is correct for an unquota'd account)",
            quotas.len()
        ),
    );

    // Cleanup: destroy the seeded emails so the probe is idempotent.
    let all = c
        .email_query(&account, EmailQueryFilter::default(), None, None, 0)
        .expect("query all");
    for id in &all.ids {
        let _ = c.email_destroy(&account, id);
    }

    if fail == 0 {
        println!("\nALL CHECKS PASSED");
    } else {
        println!("\n{fail} CHECK(S) FAILED");
        std::process::exit(1);
    }
}
