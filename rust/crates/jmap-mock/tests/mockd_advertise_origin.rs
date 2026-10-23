// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify advertised session endpoint flags on standalone `jmap-mockd`.

use std::process::Command;

mod common;

use common::Mockd;

#[test]
fn the_flag_rewrites_session_endpoints_to_the_given_origin() {
    let advertised = "http://plain-http.example";
    let mockd = Mockd::spawn(&["--advertise-origin", advertised]);

    let (status, _, body) = mockd.request("GET", "/.well-known/jmap", b"");

    assert!(status.contains("200"), "{status}");
    let session: serde_json::Value = serde_json::from_str(&body).expect("a session JSON body");
    assert_eq!(session["apiUrl"], format!("{advertised}/jmap"));
    assert_eq!(
        session["downloadUrl"],
        format!("{advertised}/download/{{accountId}}/{{blobId}}/{{name}}")
    );
    assert_eq!(
        session["uploadUrl"],
        format!("{advertised}/upload/{{accountId}}")
    );
    assert_eq!(
        session["eventSourceUrl"],
        format!("{advertised}/eventsource")
    );
}

#[test]
fn help_lists_the_advertise_origin_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_jmap-mockd"))
        .arg("--help")
        .output()
        .expect("run jmap-mockd --help");
    let stderr = String::from_utf8(output.stderr).expect("utf8 help output");

    assert!(stderr.contains("--advertise-origin"), "{stderr}");
}
