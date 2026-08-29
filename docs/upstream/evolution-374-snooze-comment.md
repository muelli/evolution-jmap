<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# DRAFT comment for GNOME/evolution#374 — "Snooze - ability to shelve a mail for later"

**Status: draft, not filed. BLOCKED on one thing — see "Before posting".**

Supersedes the earlier `evolution-mail-snooze-issue.md`, which proposed a NEW
issue on the false premise that none existed. #374 does exist; my search missed
it. Filing a new one would have been a duplicate of a *closed* report, which is
the worst version of this.

**What is known about #374 (GitLab API, 2026-08-29):**

| | |
|---|---|
| State | **closed** |
| Labels | `1. Feature`, `3. Not Actionable` |
| Created | 2019-03-26 |
| Last updated | 2025-11-23 |
| Comments | 6 |
| Upvotes | 2 |

## Before posting — I could not read the discussion

`gitlab.gnome.org` blocks automated fetching, and the 6 comments are rendered
client-side, so **the reason it was closed "Not Actionable" is unknown to me.**
That reason decides the framing, and posting without it risks re-arguing a
point a maintainer already settled. Paste the thread in and this draft can be
adjusted. Two likely cases:

- *Closed because there was no server-side mechanism in 2019* → the text below
  is directly responsive, and asking to reopen is reasonable.
- *Closed on a design objection* (e.g. "use Mark for Follow Up / Tasks", or
  "Evolution should not invent per-account UI") → then the protocol news does
  not answer it, and the right move is a short comment noting what changed and
  explicitly leaving the decision alone, **not** a reopen request.

---

## Draft comment

This was closed as Not Actionable in 2019. That looks right for the time —
there was no standardised way for a server to hold a message and put it back —
but the protocol situation has changed since, and one half of the feature now
works in Evolution already, so it may be worth a fresh look.

**There is now a standards-track mechanism.**
[`draft-ietf-extra-email-snooze`](https://datatracker.ietf.org/doc/draft-ietf-extra-email-snooze/)
(IETF EXTRA WG) defines snooze for IMAP, JMAP **and** Sieve together, so this
need not be a per-protocol special case. For JMAP it adds capability
`urn:ietf:params:jmap:mail:snooze` and an immutable `snoozed` property on
`Email` (`until`, optional `moveToMailboxId`, optional `setKeywords`); the
message rests in a mailbox with role `snoozed` and the **server** moves it back.
A companion draft registers the `Snoozed` mailbox attribute. To be clear about
its status: this is an Internet-Draft, not an RFC — reason for caution about
committing to the exact shape, but no longer "there is no mechanism".

**Evolution already does the read half, by accident.** I maintain an
out-of-tree JMAP backend, and a message snoozed elsewhere — Fastmail's web UI,
say — already behaves correctly in Evolution today: the `snoozed`-role mailbox
is a role Evolution doesn't recognise, so it maps to an ordinary folder, the
message sits there visibly, and it reappears in the Inbox at the right time
because the server does the moving. No Evolution code understands snooze for
that to work.

So what is missing is narrower than the original request implies. It is not
"implement snoozing" — it is **the action**: a way to say "snooze this until
T", and a capability check so the action only appears for accounts whose server
supports it. Everything after that is the server's job.

**Why it might still be Not Actionable, honestly:** it needs UI, and it needs
Camel/EDS to express a per-account capability plus a move-with-a-timer that
isn't well modelled by a flag. If that is the blocker rather than the protocol,
this comment doesn't change anything and I'd rather know that than push.

I'm happy to do the JMAP-side work if the Evolution-side plumbing is ever of
interest. Not asking for it to be prioritised — mainly recording that the
"there's no way to do this" part has expired.
