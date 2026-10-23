// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Components that come in through `e_cal_client_receive_objects`, through a
//! real, running `evolution-calendar-factory`.
//!
//! Evolution calls that function when the user accepts an invitation from a
//! mail, and when the user imports a file. The data comes from another person,
//! so the server must not send invitations in the user's name for it. The
//! user's answer to another organizer must still go out.
//! `tests/functional/cal-received-client.c` does the steps, and this test reads
//! the scheduling outbox of the mock.

use std::collections::BTreeMap;

use jmap_functional::{Keyfile, Session, SourceKind, observations, required_path};
use jmap_mock::MockServer;
use jmap_proto::Id;

const OLGA: &str = "mailto:olga@example.com";
const VERA: &str = "mailto:vera@example.com";
const IDA: &str = "mailto:ida@example.com";
/// Must match the addresses in `cal-received-client.c`.
const STRANGERS: [&str; 2] = [
    "mailto:first-stranger@example.net",
    "mailto:second-stranger@example.net",
];

fn keyfile(port: u16) -> String {
    Keyfile::new(SourceKind::Calendar, port).to_string()
}

#[test]
fn received_components_never_make_the_server_invite_anyone_in_the_users_name() {
    let client = required_path("JMAP_FUNCTIONAL_CAL_RECEIVED_CLIENT");
    let module = required_path("JMAP_FUNCTIONAL_CAL_MODULE");

    let server = MockServer::builder().start();
    let account_id: Id = server.account_id();
    {
        let state = server.state();
        let mut state = state.lock().expect("mock state lock");
        let account = state
            .account_mut(&account_id)
            .expect("the mock's default account");
        account.seed_participant_identity("Olga", OLGA, true);
        account.seed_calendar("Work", true);
    }
    let port: u16 = server
        .origin()
        .rsplit_once(':')
        .expect("the mock's origin ends in a port")
        .1
        .parse()
        .expect("the mock's port is a number");

    let mut session = Session::new(format!("{}/calendar-received", env!("CARGO_TARGET_TMPDIR")));
    session.write_source("jmap-functional", &keyfile(port));
    session.stage_calendar_backend(&module);

    let output = session.run(&client, &["jmap-functional"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- client stdout ---\n{stdout}--- client stderr ---\n{stderr}");
    let observed: BTreeMap<String, String> = observations(&stdout)
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    let seen = |key: &str| observed.get(key).map(String::as_str);
    assert_eq!(
        seen("connection-status"),
        Some("connected"),
        "EDS never saw the source reach connected\n{report}"
    );
    assert!(
        output.status.success(),
        "the client failed with {}\n{report}",
        output.status
    );

    let state = server.state();
    let state = state.lock().expect("mock state lock");
    let mut sent: Vec<(String, String)> = state
        .account(&account_id)
        .expect("the mock's default account")
        .scheduling_outbox
        .iter()
        .map(|message| (message.method.clone(), message.recipient.clone()))
        .collect();
    sent.sort();

    // Only the user's own create invites (Ida), and the accepted invitation
    // answers its organizer (Vera). No stranger gets a message.
    assert_eq!(
        sent,
        [
            ("REPLY".to_owned(), VERA.to_owned()),
            ("REQUEST".to_owned(), IDA.to_owned()),
        ],
        "{report}"
    );
    assert!(
        sent.iter().all(|(_, to)| !STRANGERS.contains(&to.as_str())),
        "{report}"
    );
    assert_eq!(seen("received-forged"), Some("1"), "{report}");
    assert_eq!(seen("own-changed"), Some("0"), "{report}");
    assert_eq!(seen("own-permission-denied"), Some("1"), "{report}");
    assert!(
        seen("own-message").is_some_and(|message| message.contains("You organize this meeting")),
        "{report}"
    );
}
