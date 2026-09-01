# Roadmap

Goal: a **secure, easy to use, natively integrated** way to use JMAP from
GNOME Evolution, covering mail, contacts and calendars, structured like
evolution-ews, written in Rust, developed test-first against the in-repo mock
server (`jmap-mockd`), and shipped as installable artifacts.

## Where it stands

All ten original milestones are complete and **v0.3.0 is released**, built
reproducibly with Sigstore provenance. Against a real Fastmail account, in
real Evolution: mail (read, send, folders), contacts, calendars and the
collection account all work end to end, with OAuth 2.0 sign-in, silent token
refresh, JMAP Push, server-side free/busy in the meeting scheduler, and
create/delete of calendars and address books from Evolution's own dialogs.

## Direction

Rough order of what matters next; none of it is a commitment.

- **Sharing** (JMAP Principals): reading other principals' collections and
  free/busy works; write-side sharing (`shareWith`, `ShareNotification`) is
  deliberately parked until the drafts settle.
- **Hardening against real servers**: the client is strict about RFC 8414
  issuer matching by design; deployments whose advertised hostname does not
  match how clients reach them are refused, and the docs say what to fix.
- **Upstream work this project feeds**: a named `CamelSasl` for OAuth-only
  providers (GNOME/evolution#3382), scheduled send and snooze as first-class
  Evolution features (GNOME/evolution#411, #374), and the diagnostics and
  leak reports filed from here (glib#4041, evolution-data-server!243).
- **Newer Evolution/EDS**: the code builds and tests against both the pinned
  EDS 3.52 and 3.60; a full port forward waits until the plugin's basics are
  boring.

## Contributing

`ci/checks.sh` is the single definition of green: REUSE lint, rustfmt,
clippy with warnings denied, and the full test suite. The functional suite
under `cmake/Functional.cmake` drives real EDS factories headlessly. Crates
that need EDS headers stay out of the default workspace members, so a plain
`cargo test` works without them.
