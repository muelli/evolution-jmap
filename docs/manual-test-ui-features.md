# Manual test: the JMAP UI features

`module-jmap-configuration.so` contains the account setup. It also contains
the `jmap-ui` extensions:

- a vacation autoresponder page in the account editor
- scheduled send in the composer
- snooze in the context menu of the message list
- an info bar with **Undo** after you delete a calendar or an address book

Each test needs a running Evolution, the installed module and an account whose
server offers the feature.

Each feature has two gates:

- The backend name of the account (or the Camel provider of the folder)
  decides whether the control appears.
- The JMAP session of the server decides whether the control is sensitive.

Test both gates. The control appears only on JMAP accounts. It is sensitive
only where the server supports the feature.

## What automated tests already cover

- Protocol round trips against the mock server:
  - vacation (`jmap-client/tests/vacation_response.rs`)
  - scheduled send (`jmap-ui/tests/send_later.rs`,
    `jmap-client/tests/mail_send.rs`)
  - snooze (`jmap-client/tests/mail_snooze.rs`)
- Gate logic without a display (`jmap-ui/src/session_cache.rs`,
  `jmap-ui/src/send_later/schedule.rs`).
- The FFI types that the widgets use (`evo-sys/tests/gtk.rs`,
  `evo-sys/tests/alert.rs`, `evo-sys/tests/layout.rs`).
- The text, the time and the undo of the info bar after a delete
  (`jmap-ui/tests/undo_delete.rs`).
- The placeholder for an externally hosted contact photo, in a real
  Evolution (`ci/gui-scenario/scenarios/contact-photo-fetch.py`).
- `docs/gui-smoke-test.md` checks that the controls are present against
  `jmap-mockd`.

No automated test loads the extensions into a real Evolution against a real
server. This recipe does.

## Servers

- **Stalwart** offers vacation and scheduled send. It does not offer snooze.
  Use it for the negative snooze case.
- **Fastmail** offers vacation and scheduled send. Its public JMAP API does
  not expose snooze.
- **Snooze** uses the Cyrus vendor capability. The control is sensitive only
  in two cases: against a self-hosted Cyrus with `jmap_nonstandard_extensions`
  enabled, or against the mock library with
  `MockServer::builder().snooze_extension()`. The `jmap-mockd` binary does not
  enable it.

Install the module with `cmake --install`. Then run
`evolution --force-shutdown`, so that the shell loads the new `.so`. Run
Evolution with `EVOLUTION_JMAP_LOG=trace`. The log then shows each gate
decision.
The log also shows each JMAP request and response body, with mail data. Do
this only with a test account.

## Vacation autoresponder

1. Open **Edit ▸ Accounts**, select the JMAP account and click **Edit**.
2. Find the **Vacation Responder** page in the editor. Accounts that are not
   JMAP accounts do not have this page.
3. Select the page. It fetches the current `VacationResponse`. The widgets
   become sensitive. If the server does not offer the capability, the widgets
   stay insensitive, and a line explains why.
4. Turn on *Send automatic replies*. The fields below it become sensitive.
5. Optionally set a first day and a last day. Then set a subject and a
   message.
6. Click **OK**. The page writes only if something changed.
7. Open the editor again. The page shows what the server now has.
8. In the web UI of the provider (or with `VacationResponse/get`), check that
   the autoresponder is set.
9. **Turn the autoresponder off again.** This is a live setting on a real
   mailbox.

## Scheduled send

1. Compose a message. **File ▸ Send Later** has presets (in one hour, tomorrow
   morning, next Monday morning).
2. The submenu is sensitive only if both conditions are true:
   - The transport of the From account is a JMAP transport.
   - The server advertises a `maxDelayedSend` that is not zero.

   Change the From line to an identity that is not a JMAP identity. The
   submenu becomes insensitive. Its tooltip gives the reason.
3. Pick a preset. Evolution imports the message into Drafts. It submits the
   message with an RFC 4865 `HOLDFOR`. If the server accepts the message, the
   message moves to Sent and the composer closes. If the server refuses it,
   the composer stays open.
4. Check that the server holds the submission (`undoStatus: pending`, a future
   `sendAt`). Check that the message arrives at the chosen time.
5. The sent copy appears in **Sent** when the server accepts the submission,
   before the hold ends. This is intended. On Fastmail, a copy that is filed
   in the `scheduled` mailbox stays there (see
   `jmap-ui/src/send_later/submit.rs`).

## Snooze

1. In the message list, select one or more messages. Open the context menu.
   The **Snooze** submenu has the same kind of presets.
2. The submenu is sensitive only for a folder on a JMAP account whose server
   advertises the Cyrus snooze capability. Against Stalwart and Fastmail, it
   stays insensitive with a tooltip. The trace log shows
   `snooze gate: capability fetched`. This is the expected result.
3. On a server with the capability, pick a preset. The messages move into the
   snoozed mailbox of the server (created if it does not exist) with a wake
   time. They return to the inbox at that time. The message window
   (double-click a message) has the same submenu.
4. Check that the messages come back at the wake time.

## Externally hosted contact photo

1. On the server, give a contact a photo that is a `https:` URL, not image
   data. Let Evolution sync the address book.
2. Select the contact. The preview shows the text **Externally hosted image**
   and the URL in full, in place of the photo.
3. Check the web server log. It must show no request for that URL.
4. Select a contact whose photo is image data. The preview shows that photo
   as before.

## Undo after a delete

The registry does not destroy a deleted calendar or address book at once. It
waits for one minute. In this time, an info bar shows the **Undo** button.

1. In the Calendar view, right-click a JMAP calendar and click **Delete**.
   Confirm the delete.
2. The calendar disappears from the list. An info bar shows at the top of the
   view: "Calendar "name" is deleted." For a calendar that another user shares
   with you, the text says that it is deleted for all users who share it.
3. Click **Undo**. The calendar comes back. The info bar closes.
4. On the server (or in the web UI), check that the calendar is still there.
   For a shared calendar, check it as the owner too.
5. Delete the calendar again, and do not click **Undo**. After one minute, the
   info bar closes. Check that the server no longer has the calendar.
6. Do steps 1 to 5 again in the Contacts view, with a JMAP address book.
7. Delete a calendar, then quit Evolution before the minute ends. Start
   Evolution again. If the minute did not end, the info bar shows again with
   the time that is left.
