// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! M9 layer 1, sending: the other half of the Camel provider, reached the way
//! Evolution reaches it — from the account, through the identity, to the
//! transport.
//!
//! `mail.rs` covers receiving, and stops at the store. A transport is not the
//! same object and is not configured from the same file: it is a second
//! `CamelService`, built from a second `ESource`, and the only thing that joins
//! it to the account is two hops of uid indirection through a third —
//! `[Mail Account] IdentityUid` names an identity, and that identity's
//! `[Mail Submission] TransportUid` names the transport. Nothing in Camel walks
//! that chain; Evolution does, out of `libedataserver`, and so does the client
//! program here.
//!
//! That chain is the whole reason this leg exists rather than being three more
//! assertions in `mail.rs`. Every link in it is a string in a file that no
//! compiler and no unit test can hold to the file it names, and the failure a
//! broken link produces is the quietest one this provider has:
//! `docs/manual-test-mail-provider.md` says it plainly — the account receives
//! mail perfectly and fails only when the user presses Send. A store that never
//! opens is a red account in the folder tree; a transport that was never
//! configured is a message that looks sent.
//!
//! Two tests, and the second is the failure rather than the success:
//! [`camel_sends_through_the_transport_the_identity_names`] is the chain
//! walked, connected and sent over, and
//! [`a_transport_with_no_authentication_group_cannot_send`] is the same three
//! files with the transport's `[Authentication]` group deleted — the mistake the
//! recipe warns about, made on purpose, so that the thing which catches it is a
//! test and not a reader.

use jmap_functional::{Session, observations, required_path};

/// The uids of the three sources, which are also their file names. They are
/// named here rather than derived because they are what the keyfiles below
/// point at each other with: `IdentityUid` and `TransportUid` are these
/// strings, and the client is handed only the first of the three.
const ACCOUNT_UID: &str = "jmap-functional-account";
const IDENTITY_UID: &str = "jmap-functional-identity";
const TRANSPORT_UID: &str = "jmap-functional-transport";

/// Who the account sends as. It is the identity's `Address` in the keyfile, the
/// address the client puts in the envelope, and the identity seeded on the
/// mock — three places that have to agree for `Identity/get` to resolve, and
/// the test asserts the submission went out through the seeded one.
const SENDER: &str = "alice@example.com";
const SENDER_NAME: &str = "Alice Example";

/// Who it is sent to. Deliberately not an address the mock knows anything
/// about: an envelope recipient is not a record on the server, it is a string
/// the submission carries.
const RECIPIENT: &str = "bob@example.com";

/// The message the composer would have built. Owned by this file rather than by
/// the client program so that what is asserted and what is sent are one string.
const SUBJECT: &str = "Lunch on Tuesday";
const BODY: &str = "One o'clock at the usual place.";

/// The account: what mail arrives in, and — through `IdentityUid` — the only
/// pointer that exists towards what it leaves through.
///
/// A literal, for the reason the other legs give: this is
/// `docs/examples/jmap-mock-standalone-mail.source` with the mock's ephemeral
/// port filled in, and a change to the documented recipe should fail a test
/// rather than quietly retarget one. `jmap-mail`'s `recipe.rs` is what holds
/// the documented files themselves to what they claim.
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

/// The identity: who the mail is from, and where the chain turns towards the
/// transport. It names no server of its own — an identity is not a service.
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

/// The transport: what mail leaves through, with a server of its own.
///
/// `authentication` is what makes this function interesting. The
/// `[Authentication]` group here is a *second* copy of the account's, and it has
/// to be: a store and a transport are two `CamelService`s configured from two
/// `ESource`s, and with no collection above them there is nothing that would
/// copy a server from one to the other. Passing `false` deletes it, which is the
/// mistake the second test makes.
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

/// A mock with the account a send needs, and the three paths CTest passes.
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
    /// All three mailboxes, because which two a send uses is decided from the
    /// roles the account has: `OutgoingMailboxes` stages in Drafts and files
    /// into Sent when both exist, which is the ordinary account and the one
    /// whose `out_sent_message_saved` answer is `TRUE`. An account seeded with
    /// fewer would be testing a fallback rather than the normal path.
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

    /// The mailboxes an email is in, by id rather than by name: the roles are
    /// what a send is decided from, and two mailboxes may share a name.
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

    /// Every email the account holds, which after one send is one.
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

    /// The bytes an email was imported from, as they arrived over the upload.
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

/// Runs the client and returns its output together with a report to attach to
/// any failure. The client is handed the *account* uid and nothing else about
/// the chain — finding the transport from it is what is being tested.
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

/// What the client said, with its streams already decoded.
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

    // The chain, before anything about the send: these three are what the
    // client resolved out of the registry, and a wrong one here explains every
    // later failure. `transport-uid` is the assertion the whole leg is for —
    // the client was given the account and arrived at the transport, through
    // the identity, using only what the keyfiles say.
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

    // Camel found the provider a second time, in the transport slot this time:
    // `object_types[CAMEL_PROVIDER_TRANSPORT]`, which is a different entry of
    // the same registered struct than the store came out of.
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

    // Camel's one out-parameter, and not decoration: told `0`, Evolution
    // appends a copy of its own to the account's sent folder, so a wrong answer
    // here is either two of every sent message or none of them. This account
    // has a Sent mailbox, so the copy is saved.
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

    // The other end. One submission, through the identity the mock seeded for
    // the address the *identity source* named — which is the chain checked
    // again from the server's side, and this time including `Identity/get`.
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

    // The envelope, which is what the message is *delivered* by and is not the
    // headers. The client hands Camel two `CamelAddress` lists, exactly as
    // `e_mail_session_send_to` does, and RFC 8621 §7's envelope is what has to
    // come out the far side.
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

    // And what the account holds now: one message, in Sent rather than in the
    // Drafts it was staged in, no longer a draft. That move is the server's own
    // `onSuccessUpdateEmail`, so seeing it here is evidence the submission was
    // accepted and not merely posted.
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

    // The bytes themselves, which nothing above looks at: the subject and the
    // body as Camel's own emitter wrote them out. A provider that submitted an
    // empty message would satisfy every assertion up to here.
    let source = fixture.blob(email);
    assert!(
        source.contains(SUBJECT),
        "the uploaded message has no subject line:\n{source}\n{report}"
    );
    assert!(
        source.contains(BODY),
        "the uploaded message has no body:\n{source}\n{report}"
    );

    // The requests, in the order a send makes them. `Email/import` is preceded
    // by a blob upload, which is a plain HTTP PUT and not a method call, so it
    // is not in this list — the import naming a blob is what says it happened.
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

    // The chain is intact: this is not a source the client failed to find, it
    // is one it found and that names no server. The distinction is the point —
    // the failure has to be about the transport's own configuration, not about
    // the walk.
    assert_eq!(
        seen.get("transport-uid"),
        Some(&TRANSPORT_UID),
        "the identity does not name the transport\n{report}"
    );

    assert!(
        !output.status.success(),
        "a transport with no server connected anyway\n{report}"
    );

    // At the connect, and named for what it is. `SourceError::MissingHost`'s
    // sentence is shared with the book and calendar backends, which is why it
    // says "account" rather than "transport": the user's mistake is one line
    // missing from one of their account's files, and this is the message
    // Evolution puts in front of them when they press Send.
    assert!(
        output
            .stderr
            .contains("connect: the account does not name a JMAP server"),
        "the transport failed somewhere other than the connect\n{report}"
    );

    // And it cost nothing. Not one request, so not one message imported into
    // the account for a send that never happened — the failure a user recovers
    // from by adding a line to a keyfile, rather than by deleting a draft they
    // did not write.
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
