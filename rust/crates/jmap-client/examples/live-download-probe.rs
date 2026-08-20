// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Live probe of the exact code path Evolution's mail provider uses to fetch a
// message body: `Client::connect` (honouring `JMAP_LIVE_SERVER_REBASE_URLS`,
// like every backend) followed by `Client::download_blob` — guardrail,
// download-URL templating, Accept header and all. Built to verify item 9
// against a real server without clicking through Evolution.
//
// Usage:
//   cargo run -p evolution-jmap-client --example live-download-probe -- \
//       <origin> <account-id> <blob-id> [token-file]
// e.g.
//   ... -- https://api.fastmail.com u7dbe43a0 G87eb72e... ~/.fastmail-api-token
//
// The token is read from the file (default ~/.fastmail-api-token) and never
// printed.

use jmap_client::{Client, Credentials};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(origin), Some(account), Some(blob)) = (args.next(), args.next(), args.next()) else {
        eprintln!("usage: live-download-probe <origin> <account-id> <blob-id> [token-file]");
        std::process::exit(2);
    };
    let token_file = args.next().unwrap_or_else(|| {
        format!(
            "{}/.fastmail-api-token",
            std::env::var("HOME").expect("HOME")
        )
    });
    let token = std::fs::read_to_string(&token_file)
        .unwrap_or_else(|e| panic!("cannot read token file {token_file}: {e}"))
        .trim()
        .to_owned();

    println!("rebase env active: {}", jmap_client::rebase_urls_from_env());

    let client = Client::connect(&origin, Credentials::Bearer(token)).expect("connect");
    let session = client.session();
    println!("apiUrl      = {}", session.api_url);
    println!("downloadUrl = {}", session.download_url);

    let account = jmap_proto::Id::from(account.as_str());
    let blob = jmap_proto::Id::from(blob.as_str());
    match client.download_blob(&account, &blob, "probe", 1024 * 1024) {
        Ok(bytes) => {
            let head: String =
                String::from_utf8_lossy(&bytes[..bytes.len().min(120)]).replace(['\r', '\n'], " ");
            println!("DOWNLOAD OK: {} bytes; head: {head}", bytes.len());
        }
        Err(error) => {
            println!("DOWNLOAD FAILED: {error}");
            std::process::exit(1);
        }
    }
}
