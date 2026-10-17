# Manual test: the JMAP UI features

`module-jmap-configuration.so` contains the account setup and also the
`jmap-ui` extensions: a vacation autoresponder page in the account editor,
scheduled send in the composer, and snooze in the message list context menu.
Each test needs a running Evolution, the installed module and an account
whose server offers the feature.

Each feature has two gates. The account's backend name (or the folder's Camel
provider) decides whether the control appears. The server's JMAP session
decides whether the control is sensitive. So test both: the control appears
only on JMAP accounts, and it is sensitive only where the server supports the
feature.

## What automated tests already cover

- Protocol round trips against the mock: vacation
  (`jmap-client/tests/vacation_response.rs`), scheduled send
  (`jmap-ui/tests/send_later.rs`, `jmap-client/tests/mail_send.rs`) and
  snooze (`jmap-client/tests/mail_snooze.rs`).
- Gate logic without a display (`jmap-ui/src/session_cache.rs`,
  `jmap-ui/src/send_later/schedule.rs`).
- The FFI types that the widgets use (`evo-sys/tests/gtk.rs`,
  `evo-sys/tests/layout.rs`).
- `docs/gui-smoke-test.md` checks that the controls are present against
  `jmap-mockd`.

No automated test loads the extensions into a real Evolution against a real
server. This recipe does.

## Servers

- **Stalwart** offers vacation and scheduled send, but not snooze. Use it for
  the negative snooze case.
- **Fastmail** offers vacation and scheduled send. Its public JMAP API does
  not expose snooze.
- **Snooze** uses the Cyrus vendor capability. It is only sensitive against a
  self-hosted Cyrus with `jmap_nonstandard_extensions` enabled, or against
  the mock library with `MockServer::builder().snooze_extension()`. The
  `jmap-mockd` binary does not enable it.

Install the module (`cmake --install`, then `evolution --force-shutdown` so
the shell loads the new `.so`). Run Evolution with `EVOLUTION_JMAP_LOG=trace`
to see each gate decision in the log.

## Vacation autoresponder

1. Open **Edit ▸ Accounts**, select the JMAP account and click **Edit**.
2. The editor has a **Vacation Responder** page. Non-JMAP accounts do not.
3. Select the page. It fetches the current `VacationResponse` and the widgets
   become sensitive. If the server does not offer the capability, the widgets
   stay insensitive and a line explains why.
4. Turn on *Send automatic replies*. The fields below it become sensitive.
   Optionally set a first and last day, then a subject and a message, and
   click **OK**. The page writes only when something changed. Open the
   editor again to see what the server now has.
5. Check in the provider's web UI (or with `VacationResponse/get`) that the
   autoresponder is set. **Turn it off again afterwards.** This is a live
   setting on a real mailbox.

## Scheduled send

1. Compose a message. **File ▸ Send Later** has presets (in one hour,
   tomorrow morning, next Monday morning).
2. The submenu is sensitive only when the From account's transport is JMAP
   and its server advertises a non-zero `maxDelayedSend`. Change the From
   line to a non-JMAP identity: the submenu becomes insensitive and its
   tooltip gives the reason.
3. Pick a preset. The message is imported into Drafts and submitted with an
   RFC 4865 `HOLDFOR`. When the server accepts it, the message moves to Sent
   and the composer closes. If the server refuses, the composer stays open.
4. Check that the submission is held (`undoStatus: pending`, a future
   `sendAt`) and that the message arrives at the chosen time.
5. The sent copy appears in **Sent** when the server accepts the submission,
   before the hold ends. This is intended: a copy filed in the `scheduled`
   mailbox stays there on Fastmail (see `jmap-ui/src/send_later/submit.rs`).

## Snooze

1. In the message list, select one or more messages and open the context
   menu. The **Snooze** submenu has the same kind of presets.
2. It is sensitive only for a folder on a JMAP account whose server
   advertises the Cyrus snooze capability. Against Stalwart and Fastmail it
   stays insensitive with a tooltip, and the trace log shows
   `snooze gate: capability fetched`. That is the expected result.
3. On a server with the capability, pick a preset. The messages move into the
   server's snoozed mailbox (created if absent) with a wake time, and return
   to the inbox at that time. The message window (double-click a message) has
   the same submenu.
4. Check that the messages come back at the wake time.
