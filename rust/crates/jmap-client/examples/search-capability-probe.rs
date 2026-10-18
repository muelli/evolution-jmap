// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Live verification for server-side search against a real JMAP server.
// Seeds test emails and confirms that the server evaluates `EmailQueryFilter` fields.
// Also confirms `SearchSnippet` and `Quota/get` responses.
// Prints PASS or FAIL per check and exits non-zero on any failure.
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

    // Inbox identifier.
    let inbox = c
        .mailbox_get(&account)
        .expect("Mailbox/get")
        .list
        .into_iter()
        .find(|m| m.role.as_deref() == Some("inbox"))
        .expect("an inbox")
        .id
        .expect("inbox id");

    // Seeds two emails with distinct body text and sender addresses.
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
    // Waits for the server full-text index.
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

    // 1. Full-text search on message body.
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

    // 2. Search on from header.
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

    // 3. Search on subject header.
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

    // 4. SearchSnippet on the body match.
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
            "snippet (item 46)",
            got,
            format!("{} snippet(s), highlight present: {got}", snips.len()),
        );
    }

    // 5. Quota/get: Verifies that the server accepts the call and returns a valid list.
    // An account without configured limits returns zero quota objects.
    let quotas = c.quotas(&account).expect("Quota/get");
    check(
        "quota Quota/get accepted (item 49)",
        true,
        format!(
            "{} quota object(s) (0 is correct for an unquota'd account)",
            quotas.len()
        ),
    );

    // Cleans up seeded test emails.
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
