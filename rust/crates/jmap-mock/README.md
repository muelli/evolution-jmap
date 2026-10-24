<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# evolution-jmap-mock

`evolution-jmap-mock` provides an in-memory JMAP test server and the `jmap-mockd`
binary for integration tests.

## What It Is

- `MockServer` starts a local HTTP server with mutable account state.
- `MockServerBuilder` configures auth, protocol limits, and hostile server behavior.
- `MockServer::state`, `with_account`, and `with_account_mut` let tests seed fixtures
  and inspect server results.
- `jmap-mockd` runs the same server as a standalone process for shell and Python tests.

## Modes

`jmap-mock` supports both normal and hostile modes.

- Auth and session controls: `basic_auth`, `bearer_token`, `without_capability`,
  `without_primary_accounts`.
- Limit and pagination controls: `calls_in_request`, `changes_page_size`,
  `query_page_size`, `objects_in_get`, `size_request`, `size_upload`.
- Fault injection controls through `Faults` (for example non-terminating paging,
  unrequested objects, local `file:` URI rewrites, redirected discovery, terse
  create responses).
- OAuth endpoints for integration tests: `oauth_authorization_server`,
  `oauth_client_registration`, `oauth_token`.

Run `jmap-mockd --help` for CLI flags that map to these builder modes.

## jmap-mockd from a shell

Start a server on an ephemeral port and extract the JSON listening line:

```sh
jmap-mockd --port 0 --query-never-terminates --seed ./seed.json
```

In another shell, probe the session endpoint:

```sh
curl -sS http://127.0.0.1:8080/.well-known/jmap | jq .
```

## jmap-mockd from Python

```python
import json
import subprocess
import urllib.request

proc = subprocess.Popen(
    ["jmap-mockd", "--port", "0", "--oauth2"],
    stdout=subprocess.PIPE,
    text=True,
)

line = proc.stdout.readline().strip()
origin = line.removeprefix("jmap-mockd listening on ")
session_url = f"{origin}/.well-known/jmap"
with urllib.request.urlopen(session_url) as response:
    session = json.load(response)

print(session["apiUrl"])
proc.terminate()
proc.wait()
```

## Example: test a client against the mock server

`examples/client_smoke.rs` shows a minimal client smoke test:

```rust
use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::mail::Mailbox;
use jmap_proto::session::CAPABILITY_MAIL;

fn main() {
    let server = MockServer::builder().start();
    let client = Client::connect(server.origin(), Credentials::none()).expect("connect");
    let account_id = client.primary_account(CAPABILITY_MAIL).expect("mail account");

    client
        .mailbox_create(&account_id, &Mailbox::new("Client example mailbox"))
        .expect("create mailbox");

    let mailboxes = client.mailbox_get(&account_id).expect("mailbox get");
    assert!(
        mailboxes
            .list
            .iter()
            .any(|mailbox| mailbox.name == "Client example mailbox")
    );
}
```

Run it with:

```sh
cargo run -p evolution-jmap-mock --example client_smoke
```

## Feature Flags

- `parse` (off by default): enables `ContactCard/parse` and `CalendarEvent/parse` via
  `evolution-jmap-vcard` and `evolution-jmap-ical`.

## Minimum Supported Rust Version

This crate follows the workspace MSRV: Rust 1.97.

## License

GPL-3.0-or-later.
