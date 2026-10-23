// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify shared-account name spoofing flags on standalone `jmap-mockd`.

use std::process::Command;

mod common;

use common::Mockd;

#[test]
fn hostile_flag_makes_shared_account_name_match_username() {
    let mockd = Mockd::spawn(&["--shared-account-name-copies-user"]);

    let (status, _, body) = mockd.request("GET", "/.well-known/jmap", b"");

    assert!(status.contains("200"), "{status}");
    let session: serde_json::Value = serde_json::from_str(&body).expect("a session JSON body");
    assert_eq!(session["accounts"]["A2"]["isPersonal"], false);
    assert_eq!(session["accounts"]["A2"]["name"], session["username"]);
}

#[test]
fn help_lists_the_shared_account_name_copies_user_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_jmap-mockd"))
        .arg("--help")
        .output()
        .expect("run jmap-mockd --help");
    let stderr = String::from_utf8(output.stderr).expect("utf8 help output");

    assert!(
        stderr.contains("--shared-account-name-copies-user"),
        "{stderr}"
    );
}
