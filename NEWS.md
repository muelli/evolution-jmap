<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# NEWS

## 0.5.0 (unreleased)

- Fixed a brand-new account silently signing in twice on its first
  connect to the server, caused by its own setup accidentally
  triggering a second sign-in.
- Editing a JMAP account's server address now triggers a fresh sign-in
  instead of needing Evolution restarted to take effect.
- Fixed an endless chain of sign-in prompts when an account's server
  address ends in a trailing dot (for example api.fastmail.com.). The
  dot names the same server and is now treated that way, sign-in
  credentials survive the server's redirect, and a redirect that
  cannot carry them is reported as a clear connection error instead
  of yet another consent window.
- A redirect to a malformed address from the server is now reported
  as an error instead of crashing the connection attempt.
- A new account's Sent and Drafts folders are now configured
  automatically from the server's own folder roles, the same way
  Inbox, Trash and Junk already were.
- Searching a folder's message bodies is now answered by the server,
  on evolution-data-server 3.58 and newer, instead of silently
  finding nothing.
- A mail, address book or calendar account whose saved sync state the
  server no longer recognises (for example after the server was
  restored from backup) now resynchronises automatically instead of
  getting stuck until its local cache is deleted by hand.
- On evolution-data-server 3.62 and newer, the mail cache is now
  written atomically, protecting it against corruption if Evolution
  is interrupted mid-write.
- On evolution-data-server 3.63.1 and newer, OAuth 2.0 account setup
  uses evolution-data-server's own OAuth2Dynamic support instead of
  this project's own implementation. There is no migration: an
  existing account using the old method needs to be set up again
  after upgrading to such a build.
- A round of interoperability fixes against real JMAP servers (tested
  against Stalwart): folder creation, calendar participants and
  notifications, and shared-resource ownership now tolerate several
  real-server quirks that used to produce spurious errors.
- Setting up or reconnecting several JMAP accounts at once no longer
  lets one slow or unreachable server's address lookup delay the
  others.
- Added a German translation.
- Updated the bundled TLS library (rustls) to close four published
  security advisories (RUSTSEC-2026-0346 through RUSTSEC-2026-0349).
- Mail, contacts and calendars have all been verified against a real
  JMAP server at real-world scale (up to 10,000 messages, 5,000
  contacts and 3,000 calendar events, including through
  evolution-data-server itself), with no correctness problems found.
