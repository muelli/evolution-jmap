<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# NEWS

## 0.7.1 (2026-11-23)

- Shared mailboxes: a mailbox another user shares with you now appears in
  the folder tree under a "Shared" folder, with one heading per owner.
  Reading, searching, moving, copying, deleting, expunging, subscribing,
  creating, renaming and deleting folders, and saving a message into such
  a mailbox all go to the owner's account, and a message deleted there
  lands in the owner's Trash. A read-only share is shown read-only. Shared
  folders show even when their owner has not subscribed to them. Marking a
  message seen or applying a label in a shared mailbox now updates the
  owner's message, not a different message of the same id in your own
  account; a mailbox that refuses the change keeps the mark local instead
  of erroring. The user guide lists the limits, including the shared
  read/unread flag.
- A new Delegates page in the account editor shows who may access your
  calendars and address books, and lets you grant or revoke access by
  level (Reviewer, Author, Editor, and free/busy only for calendars). A
  delegate can be typed as an email address, or picked with Find on a
  server that offers principal search. The page is read-only while
  offline.
- The Server Filters page became a real Sieve editor: syntax highlighting,
  a local syntax check that underlines the offending line and explains it
  in a tooltip, and word completion (Ctrl+Space, or automatically while
  typing, with no duplicate entry for a command that shares its own
  extension's name, and no reopening the list right after Tab accepts a
  word). Activate, Deactivate, Rename, Add and Remove, and the script
  text itself once it validates, are now written to the server at once
  instead of only on the account editor's OK, and an edit made while a
  background check is still running is no longer lost. An "Edit in
  External Editor" button opens the script in the editor configured for
  Evolution's External Editor plugin, or in $VISUAL or $EDITOR if none is
  configured. The page is read-only while offline and still shows the
  last loaded scripts when opened offline. A script the server holds that
  this page cannot interpret is refused safely instead of aborting
  Evolution. Tab now moves focus out of the editor.
- Offline mail changes are queued and replayed: a message moved or copied
  while offline shows in its new folder at once and disappears from the
  old one, survives quitting and restarting Evolution, and reaches the
  server when you are back online. Moving a message again before the first
  move has landed, copying and then moving, and moving a message with an
  unsaved flag change all queue correctly. A message saved while offline
  (for example a draft) is kept in a store that never expires and uploaded
  on reconnect. A queued change the server refuses for good is undone and
  the reason shown; a transport failure keeps it queued for the next
  attempt. See "Working offline" in the user guide.
- Push notifications now use the JMAP WebSocket channel (RFC 8887) when
  the server offers push over it, and fall back to the EventSource channel
  otherwise.
- The Vacation Responder page is read-only while offline, Tab leaves its
  message body instead of inserting a tab character, and its date fields
  work with a screen reader.
- Snoozing a message on a server that creates the Snoozed mailbox on first
  use now refreshes the folder list so the new folder shows.
- A tunnelled, self-hosted account's "rebase URLs" setting now also
  applies to its address books, calendars and mail, which used to try the
  real host the tunnel hides. Credentials are now also sent to a server
  configured with a plain http address on the same origin, such as a local
  tunnel.
- Cyrus IMAP interoperability: deleting a calendar works despite Cyrus's
  non-standard argument name, push stays off because Cyrus's push channel
  blocks every other request, and a server that answers an anonymous
  request with success instead of 401 no longer discards a valid OAuth 2.0
  refresh token.
- The client now respects the server's limits on concurrent requests and
  uploads, and refreshes a stale session before a sync pass instead of
  working from one the server no longer recognises.
- Mail cache fixes: "View Source" and header fetches could show a
  truncated message from a cache entry cut short by a crash, or a
  partially written one on evolution-data-server 3.52 to 3.61; both now
  fall back to the complete copy. A message whose server id starts with a
  dash was never cached; it is now. The folder-tree cache is written with
  private permissions.
- Labels: a keyword containing a space, a percent sign or a control
  character now round-trips exactly between the server and Evolution's
  labels instead of being split or lost. The user guide documents which
  labels and categories Thunderbird, Apple Mail, Fastmail and Roundcube
  share with Evolution.
- A mailbox whose name contains a line break can now be opened, and two
  mailboxes with empty or colliding names no longer merge into one folder.
- Push and refresh robustness: push no longer silently stops for an
  account after the system refuses to start a worker thread; a disconnect
  no longer waits on a slow server's folder listing; a late folder-listing
  answer can no longer overwrite a newer one; a message the server refused
  to update but still lists comes back on the next refresh.
- Calendar events with ten or more participants, unusual time zone names
  or odd whitespace no longer change on every sync round trip. A calendar
  event with a date far in the past or future no longer risks a crash
  from an internal overflow.
- Hardening against hostile input: control characters in an envelope
  address, a cross-origin redirect of a POST, server ids containing a
  slash, file: and data: URIs in calendar locations and attachments, a
  carriage return in a filter rule, a bidirectional override in a label,
  and a folder list thousands of siblings wide are all refused, escaped or
  capped before they reach Evolution.
- Fixed memory leaks in the client's session refresh and in the collection
  backend's source loading.
- Build and packaging: GtkSourceView 4 is a new build and runtime
  dependency (for the Sieve editor); the Flatpak extension builds it in.
  The mock server gained conformance checks for RFC 8984, 9610, 9661 and
  9670 and the JMAP calendars draft (see its own changelog). The release
  pipeline's own build image was missing the GtkSourceView 4 headers,
  which held up this release by one point version; it now carries them.

### Known issues

- Double-clicking a contact card can lose the click during an address
  book refresh (upstream Evolution bug, evolution#3436).
- On Stalwart 1.1.1, activating or deactivating a Sieve script from
  another client does not change the state JMAP reports, so the Server
  Filters page can silently undo that change on its next save. Reload
  the page after a change made elsewhere.
- Some strings are not yet translated into German.
- Snoozing a message needs a server that supports the snooze capability.
- The Delegates page shows a raw account id, instead of a name, for a
  delegate when the server has directory lookups turned off.
- A mailbox another user has unshared with you can stay in the folder
  list until the next periodic refresh, up to ten minutes later.
- Turning on the Vacation Responder on a server that allows only one
  active Sieve script at a time deactivates your own filter script, and
  turning Vacation off again does not restore it.
- The "Edit in External Editor" button on the Server Filters page can
  sit below the visible area of a small account editor window.

## 0.6.6 (2026-11-05)

- Fixed a security hole in the address book preview's "Load image" button
  for a contact's externally hosted photo: a contact could carry a forged
  button that fetched a URL of the sender's choosing when clicked, because
  the button always used the same fixed, guessable identifier. The button
  now gets a fresh, unguessable one on every render, so a forged button
  cannot be reached this way. This affects every release from 0.6.0 to
  0.6.5; upgrading is recommended.
- Removed the mail view's own "Load image" placeholder for a vCard photo
  attached to a message: it had the same forgeable-button flaw, reachable
  from a plain HTML message with no vCard attachment at all. It was never
  released. Evolution's own "Load remote content" setting already covers
  the mail case, so no replacement was added.
- The folder tree, and messages already read, now still show after
  starting Evolution with no network, or after restarting it offline.
- Deleting a message, emptying Trash, and expunging a folder now succeed
  while offline; the change reaches the server once you reconnect.
- The Vacation Responder, Server Filters, and Send Later pages now say
  "offline" right away, instead of waiting for a connection attempt to
  time out.
- A reconnect that keeps failing no longer loops forever: Evolution now
  falls back to the cached folder tree instead.
- "Download messages for offline use" and "Copy folder content locally"
  no longer redownload every message each time; only messages not
  already cached are fetched.
- Fixed a message that could disappear and reappear on every refresh
  when its own id needed sanitizing.
- Fixed marking a message Junk, and another client marking it Not Junk at
  nearly the same time, leaving both labels on the message.
- A contact synced from the server now gets a usable "File under" name by
  default, so Evolution's contact editor can save further edits to it
  (it used to refuse to enable Save while that field stayed blank).
- Fixed a calendar invitation or contact URL (ATTACH, IMAGE, CONFERENCE,
  ATTENDEE, ORGANIZER) gaining stray backslashes before a ';' or ','.
- Deleting a calendar or address book now also deletes its events or
  contacts, instead of leaving them behind on the server.
- Fixed a contact or calendar event from the server with a NUL byte in a
  field being saved as a truncated, malformed vCard or iCalendar document;
  the NUL byte is now replaced with a visible placeholder character
  instead.
- Hardened against a malicious or buggy server: a crafted address can no
  longer forge a second, fake address in the message list by breaking
  out of its angle brackets; a stray CR or LF in a subject or address is
  now replaced instead of planting a forged extra line; and a bidi
  override character in a subject or sender name can no longer change
  the reading direction of the columns beside it.
- Many small accessibility and keyboard fixes across the Server Filters
  page, its rule dialog, Vacation Responder, Send Later, snooze, and the
  undo-delete notice: fixed keyboard shortcuts that clashed with each
  other, made Enter confirm dialogs that used to ignore it, gave screen
  readers names for fields and buttons that had none and spoken
  announcements for status changes, and made every control reachable
  with the keyboard alone. Dialog spacing, capitalization, and ellipses
  now follow GNOME's own style consistently.

## 0.6.5 (2026-11-01)

- The Server Filters page no longer scrolls sideways. A long error message
  or a long Sieve line now wraps instead of making the page wider than the
  window.
- A rate-limited request now shows a plain sentence that tells you how long
  to wait. Before, it showed a raw technical message.
- Contact groups (mailing lists) from the server now show their name and
  members correctly in the Contact List Editor.
- Fixed a contact id that could clash with the server's own id on some
  servers, including Stalwart 1.0. The clash could make a new contact
  invisible to a group or to later changes.
- Fixed a new contact or calendar event that could stay invisible until a
  full resync, when another program on the same account made a change at
  nearly the same time.
- Saving the Vacation Responder, a Server Filters script, a contact, or a
  calendar event no longer risks silently overwriting a change another
  program made at the same time. The save now detects the conflict and
  handles it the normal way.
- Fixed two fast clicks on a snooze option, or on Send Later, sending the
  action twice.
- Fixed a crash that could happen when the composer, the Server Filters
  page, or the Undo notice was closed while one of its own dialogs was
  still open.
- Fixed a fetched contact photo that could briefly show in the wrong
  contact.
- Hardened the JMAP client against a hostile or broken server. It now
  rejects a response it did not ask for, a duplicate or out-of-order
  response, a replayed response, and a session state that goes backward.
  It no longer sends your credentials to a server response that redirects
  to another origin.

## 0.6.1 (2026-10-31)

- Added a `Server Filters` page to the account editor for servers that
  support JMAP Sieve filters. It lists and manages filter scripts, and
  offers a rules list for one managed script so most users never need
  to write Sieve by hand. If another program changes the filters while
  the page is open, the page does not overwrite that change.
- This release replaces 0.6.0, whose release build failed before
  publishing. See the `v0.6.1` tag message for detail.

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
