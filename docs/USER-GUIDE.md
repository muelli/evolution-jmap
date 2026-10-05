<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# User guide: adding and using a JMAP account in Evolution

This guide is for end users who want to use JMAP with Evolution.

## What this supports

After setup, one JMAP account can provide:

- mail
- address books
- calendars

JMAP-only Evolution features in this repository:

- vacation autoresponder page in account settings
- scheduled send in the composer
- snooze in message lists (only when the server advertises the snooze capability)

## Before you start

1. Install Evolution and this project's package for your distro.
2. Restart Evolution so it loads the new JMAP modules.
3. Keep your account details ready:
   - email address
   - password (for password login)
   - OAuth provider access (for OAuth login)

Unverified in a running Evolution: exact package names and install commands for every distro.

## Add a JMAP account (password flow)

1. Open Evolution.
2. Start adding a new mail account.
3. Select the account type named `JMAP`.
4. Fill in:
   - server URL field: `JMAP Session URL`
   - user field: `User`
5. Continue and save the account.

The account setup module writes these settings into the account source:

- `[Authentication] Host` from `JMAP Session URL`
- `[Authentication] User` from `User`

These values are consumed by the collection backend and the generated mail/calendar/address-book sources.

## Add a JMAP account (OAuth flow)

Use this when your provider supports OAuth 2.0 for JMAP.

1. In account setup, enter your address and JMAP details.
2. Trigger OAuth sign-in from the Evolution account flow.
3. Complete provider login and consent in the browser window.
4. Return to Evolution and finish account creation.

The implementation performs RFC 8414 discovery and dynamic client registration, then stores per-account OAuth endpoints and client data for token refresh.

If your server does not publish usable OAuth metadata, Evolution can show OAuth as unavailable for that account.

Unverified in a running Evolution: exact wording and placement of each OAuth prompt on all Evolution versions.

## What you should see after setup

A working JMAP account should appear as one account with three service areas:

- mail folders and send/receive
- contacts/address books
- calendars

The project manual test recipe confirms this end-to-end against real servers and the local mock server.

## JMAP-only features

### Vacation autoresponder

Open account settings and look for a `Vacation Responder` page for the JMAP account. You can enable automatic replies, set optional start/end dates, subject, and message.

Unverified in a running Evolution: exact widget layout on every desktop theme and Evolution release.

### Scheduled send

In the composer, open `File` then `Send Later`. Presets include one hour, tomorrow morning, and next Monday morning.

This menu is enabled only when the selected From account is JMAP and the server advertises delayed-send support (`maxDelayedSend > 0`).

### Snooze

In a message list context menu, use `Snooze` presets.

Important limit: many public JMAP servers do not expose the snooze capability to third-party clients. In that case the menu stays disabled for that account. This is expected behavior.

## Known limits

- Evolution does not have an EDS hook to surface JMAP push (`eventSourceUrl`) in this plugin architecture. The provider runs pull-based refresh instead.
- Snooze is capability-gated. Many public servers currently do not advertise the capability for API clients.
- Some UI details in this guide are marked unverified in a running Evolution where they depend on desktop/runtime specifics.

## Troubleshooting

- Recheck `JMAP Session URL` and `User` first.
- If mail authentication fails, confirm account credentials and whether OAuth or password flow is configured for that provider.
- If `Send Later` is disabled, verify the active From account is JMAP and the server supports delayed send.
- If `Snooze` is disabled, verify whether your server advertises snooze capability. Lack of capability is common and not necessarily a bug.

For developer-oriented validation steps and deeper diagnostics, see:

- `docs/manual-test-account-setup.md`
- `docs/manual-test-collection-backend.md`
- `docs/OAUTH-FASTMAIL.md`
- `docs/manual-test-ui-features.md`
