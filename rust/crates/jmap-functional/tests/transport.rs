// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Sending: the second part of the Camel provider. The test gets to it as
//! Evolution does, from the account through the identity to the transport.
//!
//! `mail.rs` tests receiving and stops at the store. A transport is a second
//! `CamelService` from a second `ESource`. A third source connects it to the
//! account through two uid references: `[Mail Account] IdentityUid` names an
//! identity, and its `[Mail Submission] TransportUid` names the transport.
//! Camel does not follow that chain. `EMailSession` (`libemail-engine`) and
//! the client program here follow it.
//!
//! That chain is the reason for a separate test. Each link is a string in a
//! file, and no compiler or unit test can check it. A broken link gives the
//! least visible failure of this provider (see
//! `docs/manual-test-mail-provider.md`): the account receives mail correctly
//! and fails only when the user presses Send. A store that does not open
//! shows a red account. A transport with no configuration shows a message
//! that looks sent.
//!
//! There are two tests.
//! [`camel_sends_through_the_transport_the_identity_names`] follows the
//! chain, connects and sends.
//! [`a_transport_with_no_authentication_group_cannot_send`] uses the same
//! three files without the `[Authentication]` group of the transport. This
//! is the mistake that the recipe warns about.

use jmap_functional::{Session, observations, required_path};

/// The uids of the three sources, which are also their file names.
/// `IdentityUid` and `TransportUid` in the keyfiles use these strings. The
/// client gets only the account uid.
const ACCOUNT_UID: &str = "jmap-functional-account";
const IDENTITY_UID: &str = "jmap-functional-identity";
const TRANSPORT_UID: &str = "jmap-functional-transport";

/// The sender of the account. It is the identity `Address` in the keyfile,
/// the envelope address of the client, and the identity on the mock. These
/// three must agree for `Identity/get` to resolve.
const SENDER: &str = "alice@example.com";
const SENDER_NAME: &str = "Alice Example";

/// The recipient. The mock does not know this address, because an envelope
/// recipient is only a string in the submission, not a server record.
const RECIPIENT: &str = "bob@example.com";

/// The message. This file, not the client, holds it, so the assertions and
/// the sent message use the same string.
const SUBJECT: &str = "Lunch on Tuesday";
const BODY: &str = "One o'clock at the usual place.";

/// The account receives the mail. Its `IdentityUid` is the only reference
/// to the transport.
///
/// This literal is `docs/examples/jmap-mock-standalone-mail.source` with the
/// ephemeral port of the mock. A change to the documented recipe makes a test
/// fail. `recipe.rs` of `jmap-mail` examines the documented files.
fn account(port: u16) -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional test\n\
         Enabled=true\n\
         \n\
         [Mail Account]\n\
         BackendName=jmap\n\
         IdentityUid={IDENTITY_UID}\n\
         \n\
         [Authentication]\n\
         Host=127.0.0.1\n\
         Port={port}\n\
         \n\
         [Security]\n\
         Method=none\n"
    )
}

/// The identity: the sender, and the link to the transport. It names no
/// server, because an identity is not a service.
fn identity() -> String {
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional identity\n\
         Enabled=true\n\
         \n\
         [Mail Identity]\n\
         Name={SENDER_NAME}\n\
         Address={SENDER}\n\
         \n\
         [Mail Submission]\n\
         TransportUid={TRANSPORT_UID}\n"
    )
}

/// The transport sends the mail, and has its own server.
///
/// The `[Authentication]` group is a *second* copy of the group of the
/// account. A store and a transport are two `CamelService`s from two
/// `ESource`s. With no collection above them, nothing copies the server from
/// one to the other. `authentication` set to `false` removes the group, as the
/// second test does.
fn transport(port: u16, authentication: bool) -> String {
    let server = if authentication {
        format!(
            "\n[Authentication]\n\
             Host=127.0.0.1\n\
             Port={port}\n\
             \n\
             [Security]\n\
             Method=none\n"
        )
    } else {
        String::new()
    };
    format!(
        "[Data Source]\n\
         DisplayName=JMAP functional transport\n\
         Enabled=true\n\
         \n\
         [Mail Transport]\n\
         BackendName=jmap\n\
         {server}"
    )
}

/// A mock with the account that a send needs, and the ids of its seeded
/// records.
struct Fixture {
    server: jmap_mock::MockServer,
    account_id: jmap_proto::Id,
    identity_id: jmap_proto::Id,
    drafts: jmap_proto::Id,
    sent: jmap_proto::Id,
    port: u16,
}

impl Fixture {
    /// Inbox, Sent and Drafts by role, and one identity for [`SENDER`].
    ///
    /// The roles of the account decide the mailboxes of a send. If Drafts and
    /// Sent exist, `OutgoingMailboxes` stages in Drafts and files into Sent,
    /// and `out_sent_message_saved` is `TRUE`. With fewer mailboxes, the test
    /// examines a fallback, not the normal path.
    fn start() -> Self {
        let server = jmap_mock::MockServer::builder().start();
        let account_id = server.account_id();

        let (identity_id, drafts, sent) = {
            let state = server.state();
            let mut state = state.lock().expect("mock state lock");
            let account = state
                .account_mut(&account_id)
                .expect("the mock's default account");

            account.seed_mailbox("Inbox", Some("inbox"));
            let sent = account.seed_mailbox("Sent", Some("sent"));
            let drafts = account.seed_mailbox("Drafts", Some("drafts"));
            let identity_id = account.seed_identity(SENDER_NAME, SENDER);
            (identity_id, drafts, sent)
        };

        let port = server
            .origin()
            .rsplit_once(':')
            .expect("the mock's origin ends in a port")
            .1
            .parse()
            .expect("the mock's port is a number");

        Self {
            server,
            account_id,
            identity_id,
            drafts,
            sent,
            port,
        }
    }

    /// The three sources, in a session rooted at `name`.
    fn session(&self, name: &str, authentication: bool) -> Session {
        let mut session = Session::new(format!("{}/{name}", env!("CARGO_TARGET_TMPDIR")));
        session.write_source(ACCOUNT_UID, &account(self.port));
        session.write_source(IDENTITY_UID, &identity());
        session.write_source(TRANSPORT_UID, &transport(self.port, authentication));
        session.stage_camel_provider(
            &required_path("JMAP_FUNCTIONAL_MAIL_MODULE"),
            &required_path("JMAP_FUNCTIONAL_MAIL_URLS"),
        );
        session
    }

    /// The mailbox ids of an email. Ids, not names, because two mailboxes can
    /// have the same name.
    fn mailboxes_of(&self, email: &jmap_proto::mail::Email) -> Vec<jmap_proto::Id> {
        email
            .mailbox_ids
            .as_ref()
            .expect("an imported email is in a mailbox")
            .iter()
            .filter(|(_, member)| **member)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// All emails of the account. After one send, there is one.
    fn emails(&self) -> Vec<jmap_proto::mail::Email> {
        let state = self.server.state();
        let state = state.lock().expect("mock state lock");
        state
            .account(&self.account_id)
            .expect("the account")
            .emails
            .iter()
            .map(|(_, email)| email.clone())
            .collect()
    }

    /// The uploaded bytes from which the server imported an email.
    fn blob(&self, email: &jmap_proto::mail::Email) -> String {
        let state = self.server.state();
        let state = state.lock().expect("mock state lock");
        let blob_id = email
            .blob_id
            .as_ref()
            .expect("an imported email has a blob");
        let blob = state
            .account(&self.account_id)
            .expect("the account")
            .blobs
            .get(blob_id)
            .expect("the blob the import named");
        String::from_utf8_lossy(&blob.data).into_owned()
    }
}

/// Runs the client and returns its output and a report for failures. The
/// client gets only the *account* uid. The test examines how it finds the
/// transport from that uid.
fn run(session: &Session) -> (Output, String) {
    let client = required_path("JMAP_FUNCTIONAL_TRANSPORT_CLIENT");
    let output = session.run(&client, &[ACCOUNT_UID, RECIPIENT, SUBJECT, BODY]);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    (
        Output {
            status: output.status,
            stdout,
            stderr,
        },
        report,
    )
}

/// The decoded output of the client.
struct Output {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

#[test]
fn camel_sends_through_the_transport_the_identity_names() {
    let fixture = Fixture::start();
    let session = fixture.session("transport", true);
    let (output, report) = run(&session);
    let seen = observations(&output.stdout);

    // Assert the chain first. The client resolved these values from the
    // registry, and a wrong value explains all later failures. `transport-uid`
    // is the main assertion: from the account, the client found the transport
    // through the identity, with only the keyfiles.
    assert_eq!(
        seen.get("identity-uid"),
        Some(&IDENTITY_UID),
        "the account does not name the identity\n{report}"
    );
    assert_eq!(
        seen.get("identity-address"),
        Some(&SENDER),
        "the identity does not carry the address the account sends as\n{report}"
    );
    assert_eq!(
        seen.get("transport-uid"),
        Some(&TRANSPORT_UID),
        "the identity's submission extension does not name the transport\n{report}"
    );
    assert_eq!(
        seen.get("protocol"),
        Some(&"jmap"),
        "the transport source names a protocol the provider does not register\n{report}"
    );

    // Camel found the provider again, in the transport slot
    // `object_types[CAMEL_PROVIDER_TRANSPORT]`. The store uses a different
    // entry of the same registered struct.
    assert_eq!(
        seen.get("transport-connected"),
        Some(&"1"),
        "Camel never connected the transport\n{report}"
    );

    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    assert_eq!(
        seen.get("sent"),
        Some(&"1"),
        "the send did not report success\n{report}"
    );

    // The Camel out-parameter. For `0`, Evolution adds its own copy to the
    // sent folder of the account. A wrong value gives two copies of each sent
    // message, or none. This account has a Sent mailbox, so the copy is saved.
    assert_eq!(
        seen.get("sent-copy-saved"),
        Some(&"1"),
        "the transport did not claim the sent copy it left in Sent\n{report}"
    );

    assert_eq!(
        seen.get("transport-disconnected"),
        Some(&"1"),
        "the transport did not let go of its connection\n{report}"
    );

    // The other end. One submission, through the mock identity for the
    // address of the *identity source*. This examines the chain again from the
    // server side, with `Identity/get`.
    let (email_id, identity_id, envelope) = {
        let state = fixture.server.state();
        let state = state.lock().expect("mock state lock");
        let outbox = &state
            .account(&fixture.account_id)
            .expect("the account")
            .outbox;
        assert_eq!(outbox.len(), 1, "one accepted submission\n{report}");
        let recorded = outbox.first().expect("the submission just counted");
        (
            recorded.email_id.clone(),
            recorded.identity_id.clone(),
            recorded.envelope.clone(),
        )
    };

    assert_eq!(
        identity_id, fixture.identity_id,
        "the submission did not go out through the account's identity\n{report}"
    );

    // The envelope *delivers* the message. It is not the headers. The client
    // gives Camel two `CamelAddress` lists, as `e_mail_session_send_to` does.
    // The server must get the envelope of RFC 8621 §7.
    assert_eq!(
        envelope.mail_from.email, SENDER,
        "the envelope sender is not the identity's address\n{report}"
    );
    assert_eq!(
        envelope
            .rcpt_to
            .iter()
            .map(|address| address.email.as_str())
            .collect::<Vec<_>>(),
        vec![RECIPIENT],
        "the envelope recipients are not the ones Camel was given\n{report}"
    );

    // The account now has one message, in Sent and not in Drafts, and not a
    // draft. The server `onSuccessUpdateEmail` does this move. Thus the server
    // accepted the submission.
    let emails = fixture.emails();
    assert_eq!(
        emails.len(),
        1,
        "the account holds {} messages, not the one that was sent\n{report}",
        emails.len()
    );
    let email = emails.first().expect("the message just counted");
    assert_eq!(
        email.id.as_ref(),
        Some(&email_id),
        "the submission names a message the account does not hold\n{report}"
    );
    assert_eq!(
        fixture.mailboxes_of(email),
        vec![fixture.sent.clone()],
        "the sent message was not filed into Sent ({}); it was staged in Drafts ({})\n{report}",
        fixture.sent,
        fixture.drafts
    );
    let keywords = email.keywords.clone().unwrap_or_default();
    assert!(
        !keywords.contains_key("$draft"),
        "the sent message is still a draft: {keywords:?}\n{report}"
    );

    // The subject and the body in the bytes from Camel. An empty message
    // passes all assertions above.
    let source = fixture.blob(email);
    assert!(
        source.contains(SUBJECT),
        "the uploaded message has no subject line:\n{source}\n{report}"
    );
    assert!(
        source.contains(BODY),
        "the uploaded message has no body:\n{source}\n{report}"
    );

    // The requests of a send. A blob upload (a plain HTTP POST, not a method
    // call) comes before `Email/import`, so it is not in this list. The blob
    // in the import proves the upload.
    let calls = fixture.server.method_calls();
    for method in ["Identity/get", "Mailbox/get", "Email/import"] {
        assert!(
            calls.iter().any(|call| call == method),
            "the transport never asked for {method}; it asked for {calls:?}\n{report}"
        );
    }
    assert_eq!(
        calls.last().map(String::as_str),
        Some("EmailSubmission/set"),
        "the submission is not the last thing the send did: {calls:?}\n{report}"
    );
}

#[test]
fn a_transport_with_no_authentication_group_cannot_send() {
    let fixture = Fixture::start();
    let session = fixture.session("transport-no-server", false);
    let (output, report) = run(&session);
    let seen = observations(&output.stdout);

    // The chain is correct. The client found the transport source, which
    // names no server. The failure must come from the transport
    // configuration, not from the chain.
    assert_eq!(
        seen.get("transport-uid"),
        Some(&TRANSPORT_UID),
        "the identity does not name the transport\n{report}"
    );

    assert!(
        !output.status.success(),
        "a transport with no server connected anyway\n{report}"
    );

    // The failure is at the connect. The book and calendar backends use the
    // same `SourceError::MissingHost` text, so it says "account", not
    // "transport". Evolution shows this message when the user presses Send.
    assert!(
        output
            .stderr
            .contains("connect: the account does not name a JMAP server"),
        "the transport failed somewhere other than the connect\n{report}"
    );

    // No request, so no imported message for a failed send. The user adds a
    // line to a keyfile, and does not have to delete an unwanted draft.
    let calls = fixture.server.method_calls();
    assert!(
        calls.is_empty(),
        "a transport that never connected still reached the server: {calls:?}\n{report}"
    );
    assert!(
        fixture.emails().is_empty(),
        "a send that failed at the connect left a message behind\n{report}"
    );
}
