// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

#![no_main]

use libfuzzer_sys::fuzz_target;

use jmap_client::eventsource::{parse_chunked_stream, parse_response_head, parse_stream};
use jmap_client::transport::CancelFlag;

fuzz_target!(|data: &[u8]| {
    let cancel = CancelFlag::new();
    let (tx, rx) = std::sync::mpsc::channel();

    let _ = parse_stream(std::io::BufReader::new(data), &cancel, &tx);
    while rx.try_recv().is_ok() {}

    let _ = parse_chunked_stream(std::io::BufReader::new(data), &cancel, &tx);
    while rx.try_recv().is_ok() {}

    let _ = parse_response_head(std::io::BufReader::new(data));
});
