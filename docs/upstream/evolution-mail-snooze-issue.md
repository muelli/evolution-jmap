<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# DRAFT new issue for GNOME/evolution — mail snooze

**Status: draft, not filed.** Review before posting. Unlike scheduled send
(GNOME/evolution#411, which exists), no GitLab issue for *mail* snooze was
found — only evolution-list threads (Feb 2019, Aug 2021). Note
evolution-data-server#301 is about snoozing *calendar alarms* and is unrelated.
Search once more before filing, in case one has been opened since.

---

## Title

Snooze a message until a later time (protocol support exists in JMAP)

## Description

Snoozing — hide a message now, have it return to the Inbox at a chosen time —
has been asked for on evolution-list at least twice (Feb 2019, Aug 2021). The
answer then was reasonably "use Mark for Follow Up, or make a Task", which
solves reminding but not the actual request: the message stays in the Inbox,
so it keeps costing attention.

Filing this because the protocol side has moved since those threads, and one
half of it already works in Evolution by accident.

**Protocol status.** `draft-ietf-extra-email-snooze` (IETF EXTRA WG) defines
snooze for IMAP, JMAP and Sieve together. For JMAP it adds capability
`urn:ietf:params:jmap:mail:snooze` and an immutable `snoozed` property on
`Email` of type `SnoozeDetails`:

- `until` (UTCDate) — when to move it back
- `moveToMailboxId` (String, optional) — where to; Inbox if unset
- `setKeywords` (String[Boolean], optional) — keywords to apply on waking

The message rests in a mailbox with role `snoozed` meanwhile, and the *server*
performs the awakening. **This is an Internet-Draft, not an RFC** — worth
knowing before anyone commits to it; the shape could still change.

**The read half already works.** With a JMAP account, a message snoozed
elsewhere (say Fastmail's web UI) shows up in Evolution correctly today: the
`snoozed` mailbox is an unrecognised role, so it maps to an ordinary folder,
the message is visible there, and it reappears in the Inbox at the right time
because the server moves it. Nothing in Evolution has to understand snooze for
that to work.

**What is missing is the action.** There is no way to snooze *from* Evolution,
and no plumbing to carry "until this time, then move here". As with #411, the
capability exists at the protocol layer and is unreachable from the UI.

## What this would need

1. A UI affordance — a "Snooze until…" message action with the usual presets.
2. A Camel/EDS way to express it. Snooze is closer to a move-with-a-timer than
   to a flag, and the awakening is the server's job for backends that support
   it, so a flag alone probably will not model it.
3. A capability check, so the action only appears for accounts whose server
   advertises it. IMAP has a parallel mechanism in the same draft; Exchange/EWS
   has no equivalent that I could find (Outlook implements snooze client-side),
   so this would be per-account, not global.

I maintain an out-of-tree JMAP backend (`evolution-jmap`) and am happy to do
the JMAP-side work if there is interest in the Evolution-side plumbing — but
the UI and the Camel API shape are maintainer calls, and the spec being a draft
may well be reason to wait.
