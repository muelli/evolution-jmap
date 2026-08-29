<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# DRAFT comment for GNOME/evolution#374 — "Snooze - ability to shelve a mail for later"

**Status: draft, ready for review.** #374 is CLOSED (`1. Feature`,
`3. Not Actionable`), opened 2019-03-26, last touched 2025-11-23.

This replaces an earlier draft that proposed a *new* issue — a false premise;
#374 exists and filing again would have duplicated a closed report.

**Why a reopen request is defensible here:** every blocker actually stated in
the thread has since expired, and each can be answered by name rather than by
general enthusiasm.

| Thread comment | Then | Now |
|---|---|---|
| aklapper (2019): "requires server side public API" | correct — none existed | `draft-ietf-extra-email-snooze` defines exactly that |
| aklapper (2019): design questions about client-local hiding | fatal for a local implementation | dissolve entirely under a server-side model |
| pwithnall (2021): "might now be possible … sieve-snooze" | on the right track | the WG now has the broader IMAP+JMAP+Sieve document |
| jfft (2023): "not already in the spec" for JMAP | accurate in **June** 2023 | WG draft published **October** 2023, four months later |
| jfft (2023): "Evolution does not implement JMAP" | true then | an out-of-tree backend exists now |
| anna (2025): asked to reopen | — | unanswered |

Tone note: aklapper's objection was *right*, and jfft's statement was *true
when written*. The draft says so explicitly. Nothing here should read as a
gotcha, and the ask is deliberately weak — reopen for tracking, not "please
build this".

---

## Draft comment

Replying mainly to @anna's question about reopening, since I think the answer
has actually changed. Each of the concrete objections in this thread was
correct when it was written; what follows is only what has moved since.

**@aklapper's original point — that this needs a server-side public API and so
isn't client work — was the right call in 2019, and it is the thing that has
changed.** The IETF EXTRA working group now has
[`draft-ietf-extra-email-snooze`](https://datatracker.ietf.org/doc/draft-ietf-extra-email-snooze/)
("Snoozing Email with IMAP, JMAP, and Sieve"), which defines snooze across all
three at once, plus a companion draft registering the `Snoozed` mailbox
attribute. @pwithnall was pointing in this direction in 2021 with
`draft-ietf-extra-sieve-snooze`; the working group has since produced the
broader document. For JMAP it is capability `urn:ietf:params:jmap:mail:snooze`
and an immutable `snoozed` property on `Email` — `until`, an optional
`moveToMailboxId`, and optional `setKeywords` — with the message resting in a
mailbox of role `snoozed` until the **server** moves it back.

**A small correction on the JMAP status.** @jfft noted in June 2023 that this
was proposed for JMAP but "not already in the spec", citing jmapio/jmap#301 —
accurate at the time. The working-group draft appeared that October, about four
months later, so that particular blocker has lapsed. (Also, for the record:
Evolution *can* speak JMAP now — I maintain an out-of-tree backend. It is one
person's project and not something GNOME ships, so I mention it only because
the thread assumed JMAP was permanently out of scope.)

**@aklapper's design questions were the strongest part of this thread, and
server-side snooze answers all four** — they were unanswerable for a
client-local implementation, which I think is precisely why this was closed:

- *How would it sync to other IMAP clients?* It doesn't need to. The server
  moves the message; every client sees the same thing, Evolution included, with
  no shared client-side state.
- *Should it vanish from local search?* It is in a different mailbox, exactly
  like any other folder. No special-casing of search, no invisible messages.
- *Sorted by date, how would you even notice it came back?* The draft's
  `setKeywords` lets the snooze specify keywords to apply on waking — marking
  it unread on return is expressible in the protocol rather than being an
  Evolution invention.
- *Would snoozing imply Unread?* Same answer: the client chooses, per snooze.

**On adapting "Mark for Follow Up"** (@jfft): that solves a real and adjacent
problem, but a different one — it reminds you about a message that is still
sitting in the Inbox, whereas the request here is to get it *out of view*
until a time. With the server-side mechanism the message genuinely leaves the
Inbox, so no flag semantics are needed. (On the related question of whether
IMAP can store arbitrary metadata: it can, via keywords/annotations, but the
draft deliberately specifies a mailbox move rather than ad-hoc metadata,
which is what makes it interoperable.)

**One concrete data point: Evolution already does the read half today, by
accident.** With a JMAP account, a message snoozed elsewhere — Fastmail's web
UI, say — already behaves correctly in Evolution: the `snoozed`-role mailbox is
a role Evolution doesn't recognise, so it degrades to an ordinary folder, the
message is visible there, and it returns to the Inbox on time because the
server moves it. No Evolution code understands snooze for that to work.

So the remaining gap is narrower than the original report implies. Not
"implement snoozing" — just **the action** ("snooze until T"), plus a
capability check so it only appears for accounts whose server supports it.

**Two honest caveats.** It is still an Internet-Draft, not an RFC, so the exact
shape can change — a fair reason to wait rather than build now. And the real
cost is Evolution-side anyway: UI, plus somewhere in Camel/EDS to express a
per-account capability and a move-with-a-timer, which a flag doesn't model
well. If *that* is the blocker rather than the protocol, then this stays Not
Actionable and I'd genuinely rather know.

Not asking for prioritisation — suggesting it may now be worth reopening for
tracking, since the "no server-side API exists" reason has expired. Happy to do
the JMAP-side work if the Evolution-side plumbing ever becomes interesting.
