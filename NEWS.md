<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# NEWS

## Unreleased

- Added a `Server Filters` page to the account editor for servers that
  support JMAP Sieve filters. It lists and manages filter scripts, and
  offers a rules list for one managed script so most users never need
  to write Sieve by hand. If another program changes the filters while
  the page is open, the page does not overwrite that change.

## 0.6.0 (2026-10-30)

- Calendars and address books that another user shares are now shown
  as additional collections, so they can be browsed and, with the
  right permissions, edited. Deleting a shared calendar or address
  book now acts on the correct account and is only offered when the
  user actually has permission to remove it.
- An address book or calendar without write access now opens
  read-only, so Evolution greys out editing instead of letting a save
  fail afterwards.
- A shared calendar or address book's display name can no longer be
  set by its owner to hide who shared it: the name is now always
  "owner: collection name".
- The server now sends meeting invitations, updates and cancellations
  itself. A clear error is shown when the server cannot do this for
  one of the guests, and Evolution's own "do not send invitation"
  option still suppresses a single change. A plain import or backup
  restore of a meeting no longer mails its organizer by mistake, and
  neither does removing an invitation that was already cancelled.
- Dismissing a calendar reminder is now sent to the server, so the
  same reminder does not fire again on your other devices.
- Editing a contact or event that was changed elsewhere at the same
  time now follows Evolution's own conflict choice (keep mine, keep
  the server's, or fail) instead of silently overwriting the other
  change.
- An "Undo" notice now appears after deleting a calendar or address
  book collection.
- Contact photos are now uploaded and downloaded through the server's
  blob storage, handling larger images without encoding them inline.
  You are told when a photo is too large for the server to store.
- Added a "Load image" option for an externally hosted (http or
  https) contact photo in the address book preview: a placeholder and
  the URL are shown instead of loading the image automatically, and
  the photo is fetched only on request.
- Snooze is now reachable by right-clicking a message's body, not
  only through its menu button, including in a detached message
  window. Fixed the "Next Monday Morning" snooze preset landing on
  the wrong day.
- Fixed newly created mail folders becoming invisible on servers that
  default new folders to unsubscribed.
- Fixed a deadlock that could freeze Evolution while an address book
  refreshed in the background.
- Fixed a pushed folder change (for example, creating the Snoozed
  folder) sometimes jumping the mail view to a different account.
- Deleting a message now moves it to Trash as usual; it is only
  permanently removed when Trash is emptied.
- Fixed Send Later sometimes choosing the wrong outgoing identity.
- Large batches of changes that exceed a server's per-request limit
  are now split automatically instead of failing outright.
- Server rate-limit responses are now retried automatically with a
  backoff, without duplicating any writes.
- The JMAP client now refuses a server redirect that would drop the
  connection from https to http, so credentials are never sent over a
  plain connection.
- Updated the bundled TLS library (rustls-webpki, webpki-roots) to
  close two published security advisories (RUSTSEC-2026-0381 and
  RUSTSEC-2026-0382).
- Updated the German translation (fully translated, no missing or
  fuzzy strings).

## 0.5.0 (2026-10-15)

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
