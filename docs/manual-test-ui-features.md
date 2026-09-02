# Manual test: the JMAP UI features

`module-jmap-configuration.so` carries, besides the account-setup pieces, the
`jmap-ui` extensions: a vacation-autoresponder page in the account editor,
scheduled send in the composer, and snooze in the message-list context menu.
None of these is a `cargo test` — each needs a running Evolution, an installed
module, and an account whose server actually offers the feature. This is that
recipe.

Each feature is gated twice. Whether its control appears at all is a
synchronous check on the account's backend name or the folder's Camel
provider; whether the control is *sensitive* is the server's own answer,
fetched from the JMAP session document off the main loop. So the interesting
half of every test is not "does the button work" but "is the button offered,
and only where the server can honour it".

## What is already checked without you

- The protocol layer round-trips against the stateful mock: vacation get/set
  (`jmap-client/tests/vacation_response.rs`), scheduled send with a `HOLDFOR`
  hold staged in Drafts and moved to Sent on acceptance
  (`jmap-ui/tests/send_later.rs`, `jmap-client/tests/mail_send.rs`), and snooze
  through the Cyrus vendor capability with the two server-side refusals
  (`jmap-client/tests/mail_snooze.rs`).
- The gate arithmetic is unit-tested without a display
  (`jmap-ui/src/session_cache.rs`, `.../send_later/schedule.rs`).
- The FFI surface every widget and window crosses is pinned against the running
  type system (`evo-sys/tests/{gtk,layout}.rs`).

What no automated test covers is the last foot: the extensions loading into a
real Evolution, the controls appearing on the right accounts, and the round
trip reaching a real server. That is this document.

## Servers

- **Stalwart** offers vacation and scheduled send, and *not* snooze — the
  useful negative case for the snooze gate. Any Stalwart deployment is fine.
- **Fastmail** offers all three (snooze is its own Cyrus extension). A Fastmail
  account is the only way to see the snooze item sensitive.

Install the module the usual way (`cmake --install`, then
`evolution --force-shutdown` so the shell reloads the `.so`), and run with
`EVOLUTION_JMAP_LOG=trace` to see each gate's decision.

## Vacation autoresponder

1. Open **Edit ▸ Accounts**, select the JMAP account, **Edit**.
2. A **Vacation Responder** page is in the editor's side list. Non-JMAP
   accounts do not get one.
3. Selecting it fetches the current `VacationResponse`; the widgets fill and
   become sensitive. A server that does not offer the capability leaves them
   insensitive with a one-line explanation instead.
4. Toggle *Send automatic replies*, set a first/last day (`YYYY-MM-DD`, either
   may be blank), a subject and a message, and press **OK**. Only a changed
   page writes; re-opening the editor shows what the server now holds.
5. Confirm out of band (the provider's web UI, or `VacationResponse/get`) that
   the autoresponder is set. **Turn it back off afterwards** — this is a live
   setting on a real mailbox.

## Scheduled send

1. Compose a message. **File ▸ Send Later** carries three presets (one hour,
   tomorrow morning, next Monday morning).
2. The submenu is sensitive only when the From account's transport is JMAP and
   its server advertises a non-zero `maxDelayedSend`. Switch the From line to a
   non-JMAP identity and it desensitises, with the reason in its tooltip; the
   trace logs each `send-later gate` decision.
3. Pick a preset. The composer hands over its finished message, which is
   imported into Drafts and submitted with a `HOLDFOR`; on the server's
   acceptance it moves to Sent and the composer closes.
4. Confirm the message is held (`undoStatus: pending`, a future `sendAt`) and
   then delivered at the chosen time. A refusal keeps the composer open and
   names the Drafts residue.

## Snooze

1. In the message list, select one or more messages and open the context menu.
   A **Snooze** submenu carries the same three presets.
2. It is sensitive only for a folder on a JMAP account whose server offers the
   snooze extension — Fastmail. On Stalwart (or any server without it) the item
   stays insensitive, tooltip saying so; the trace logs `snooze gate:
   capability fetched snooze=false`.
3. Pick a preset. The selected messages move into the server's snoozed mailbox
   (created if absent) with a wake time; they leave the inbox and return to it
   when the time comes. The detached message window (double-click a message)
   carries the same item.
4. Verify against the provider's own snooze UI that the message is snoozed, and
   that it reappears at the wake time.

Relates to `docs/gui-smoke-test.md` (the automated Xvfb tier, which asserts the
controls are *present* against `jmap-mockd`; this recipe is the live half it
cannot be).
