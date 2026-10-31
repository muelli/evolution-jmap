# Roadmap

Goal: a secure, easy-to-use and natively integrated way to use JMAP from GNOME
Evolution for mail, contacts and calendars. The structure of the project is
like evolution-ews. The code is in Rust. Development is test-first against the
mock server in the repository (`jmap-mockd`).

## Where it stands

v0.6.1 was released on 2026-10-31. Mail, contacts, calendars and the
collection account work end to end in Evolution against real JMAP servers.
This includes OAuth 2.0 sign-in, JMAP Push and server-side free/busy in the
meeting scheduler. It also includes the creation and deletion of calendars and
address books from the own dialogs of Evolution. `NEWS.md` lists the changes
in each release.

## Next

This list shows the rough order. No item is a commitment.

- **Meeting attendees**: keep attendee lists and replies in sync with the
  invitations that the server sends.
- **Shared calendars and address books**: show the collections that other
  principals share with the user. If the user has no write rights, show them
  read-only.
- **JMAP-only UI features** (`jmap-ui`): vacation autoresponder, scheduled
  send and snooze. They ship as an out-of-tree Evolution module, in the same
  way as the Out-of-Office page of evolution-ews. To make them part of
  Evolution itself, an upstream request exists (GNOME/evolution#411, #374).
- **Upstream work**: a named `CamelSasl` for OAuth-only providers
  (GNOME/evolution#3382).
- **Newer Evolution/EDS**: the project builds and tests the code against the
  pinned EDS 3.52. The `eds-version-matrix` CI job also tests a newer Fedora
  EDS.

## Contributing

`ci/checks.sh` is the single definition of green. It runs these checks:

- REUSE lint
- rustfmt
- clippy with warnings denied
- the test suite

The functional suite in `cmake/Functional.cmake` drives real EDS factories
headlessly. Crates that need EDS headers are not default workspace members.
A plain `cargo test` therefore works without the headers.
