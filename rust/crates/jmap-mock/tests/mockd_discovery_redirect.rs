// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Verify discovery redirect flags on standalone `jmap-mockd`.

use std::process::Command;

mod common;

use common::Mockd;

fn header_value<'a>(headers: &'a [String], name: &str) -> Option<&'a str> {
    headers.iter().find_map(|header| {
        let (key, value) = header.split_once(':')?;
        if key.eq_ignore_ascii_case(name) {
            Some(value.trim())
        } else {
            None
        }
    })
}

#[test]
fn the_flag_redirects_well_known_to_the_given_origin() {
    let mockd = Mockd::spawn(&["--well-known-redirect-to", "http://foreign.example"]);

    let (status, headers, _) = mockd.request("GET", "/.well-known/jmap", b"");

    assert!(status.contains("307"), "{status}");
    assert_eq!(
        header_value(&headers, "Location"),
        Some("http://foreign.example/jmap/session")
    );
}

#[test]
fn help_lists_the_well_known_redirect_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_jmap-mockd"))
        .arg("--help")
        .output()
        .expect("run jmap-mockd --help");
    let stderr = String::from_utf8(output.stderr).expect("utf8 help output");

    assert!(stderr.contains("--well-known-redirect-to"), "{stderr}");
}
