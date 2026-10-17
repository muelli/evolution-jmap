# Roadmap

Goal: a secure, easy to use, natively integrated way to use JMAP from GNOME
Evolution for mail, contacts and calendars. The project is structured like
evolution-ews, written in Rust, and developed test-first against the in-repo
mock server (`jmap-mockd`).

## Where it stands

v0.5.0 was released on 2026-10-15. Mail, contacts, calendars and the
collection account work end to end in Evolution against real JMAP servers.
This includes OAuth 2.0 sign-in, JMAP Push, server-side free/busy in the
meeting scheduler, and creating and deleting calendars and address books from
Evolution's own dialogs. See `NEWS.md` for the changes in each release.

## Next

This is the rough order. None of it is a commitment.

- **Meeting attendees**: keep attendee lists and replies in sync with the
  invitations that the server sends.
- **Shared calendars and address books**: show collections that other
  principals share with the user, read-only when the user has no write
  rights.
- **JMAP-only UI features** (`jmap-ui`): vacation autoresponder, scheduled
  send and snooze ship as an out-of-tree Evolution module, the way
  evolution-ews ships its Out-of-Office page. Making them part of Evolution
  itself (GNOME/evolution#411, #374) is an upstream request.
- **Upstream work**: a named `CamelSasl` for OAuth-only providers
  (GNOME/evolution#3382).
- **Newer Evolution/EDS**: the code is built and tested against the pinned
  EDS 3.52. The `eds-version-matrix` CI job also tests a newer Fedora EDS.

## Contributing

`ci/checks.sh` is the single definition of green: REUSE lint, rustfmt, clippy
with warnings denied, and the test suite. The functional suite in
`cmake/Functional.cmake` drives real EDS factories headlessly. Crates that
need EDS headers are not default workspace members, so a plain `cargo test`
works without them.
