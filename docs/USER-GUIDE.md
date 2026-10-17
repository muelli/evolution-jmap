<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# User guide: adding and using a JMAP account in Evolution

This guide is for end users who want to use JMAP with Evolution.

## Install the package

Install one package format, then restart Evolution so it reloads modules.

Debian or Ubuntu (`.deb` release asset):

```bash
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify evolution-jmap_*_amd64.deb --repo muelli/evolution-jmap
sudo apt install ./evolution-jmap_*_amd64.deb
evolution --force-shutdown
```

`gh attestation verify` needs GitHub CLI authentication (`gh auth login` or
`GH_TOKEN`). Without it, use the cosign steps in `docs/verifying-artifacts.md`.

Fedora: releases have no `.rpm`. Build one with `ci/rpm.sh` (see
`docs/PACKAGING.md`), then:

```bash
sudo dnf install ./build-rpm/*.rpm
evolution --force-shutdown
```

## Add a Fastmail account (OAuth)

Use this flow for Fastmail and other servers that support OAuth 2.0. No
pre-registered client is needed: the module registers itself with the server
through OAuth dynamic client registration (RFC 7591), which Fastmail offers.
OAuth 2.0 can only be set up through the lookup below, not on the manual
server settings page.

1. Open Evolution and add a new mail account.
2. On the identity page, enter your Fastmail address.
3. Click `Look Up Account Details`, then pick the `JMAP` result.
4. On `Receiving Email`, confirm:
   - `Server`: `api.fastmail.com`
   - `Port`: `443` (or blank, which means default HTTPS)
   - `Username`: your Fastmail address
   - `Authentication`: `OAuth 2.0`
   - `Use a secure connection (TLS)`: enabled
5. Complete browser login and consent, then return to Evolution and finish.

If the lookup does not offer OAuth for that server, use `Password` or
`API Token` authentication instead.

## Add a Stalwart account (password)

Use this flow for Stalwart and other servers with password or API token
sign-in.

1. Open Evolution.
2. Start adding a new mail account.
3. Select the account type named `JMAP`.
4. Fill in:
   - `Server`: your Stalwart host or IP (for example `<stalwart-ip>`)
   - `Port`: `443` for HTTPS, or your custom port
   - `Username`: your Stalwart login
   - `Authentication`: `Password` (or `API Token`)
   - `Use a secure connection (TLS)`: enabled. Plain HTTP is accepted only
     for a loopback address (`127.0.0.1`, for local testing).
5. Continue and save the account.

The account setup module writes these settings into the account source:

- `[Authentication] Host` from `Server`
- `[Authentication] User` from `Username`

The collection backend reads them and creates the mail, calendar and address
book sources from them.

## What you should see after setup

A working JMAP account should appear as one account with three service areas:

- mail folders and send/receive
- contacts/address books
- calendars


## Token rejected: what to expect

If an OAuth token is revoked or otherwise rejected, Evolution asks you to sign in again.

Mail uses separate receive and send services, so a rejected token can trigger one consent prompt per service. This is expected from EDS authentication flow, not a duplicate-account bug.

If prompts loop forever without a successful sign-in, treat it as an error and capture debug logs.

## Trailing-dot host case

Hosts with one trailing dot are normalized to the same host without the dot. Example: `api.fastmail.com.` is treated as `api.fastmail.com`.

You do not need to change existing accounts.

## JMAP-only features

### Vacation autoresponder

Open account settings and look for a `Vacation Responder` page for the JMAP account. You can enable automatic replies, set optional start/end dates, subject, and message.


### Scheduled send

In the composer, open `File` then `Send Later`. Presets include in one hour, tomorrow morning and next Monday morning.

This menu is enabled only when the selected From account is JMAP and the server advertises delayed-send support (`maxDelayedSend > 0`).

### Snooze

In a message list context menu, use `Snooze` presets.

Snooze needs the Cyrus snooze capability. Stalwart and Fastmail do not offer it, so the menu stays disabled for their accounts. This is expected.

## Known limits

- Snooze works only on servers that offer the Cyrus snooze capability.
- Mail uses JMAP Push when the server advertises an `eventSourceUrl`.
  Otherwise new mail arrives on the account's refresh interval.

## Logs and debug output

- Runtime logs are emitted through journald when available, otherwise stderr.
- For detailed module logs, start Evolution with `EVOLUTION_JMAP_LOG=debug`.
- For additional GLib process logs, also set `G_MESSAGES_DEBUG=all`.
- To watch live user-session logs: `journalctl --user -f`.
- Account source files are in `~/.config/evolution/sources/`.

## Uninstall

Debian or Ubuntu:

```bash
sudo apt remove evolution-jmap
evolution --force-shutdown
```

Fedora:

```bash
sudo dnf remove evolution-jmap
evolution --force-shutdown
```

## Troubleshooting

- Check `Server`, `Port` and `Username` first.
- If mail authentication fails, confirm account credentials and whether OAuth or password flow is configured for that provider.
- If `Send Later` is disabled, verify the active From account is JMAP and the server supports delayed send.
- If `Snooze` is disabled, verify whether your server advertises snooze capability. Lack of capability is common and not necessarily a bug.

For developer test steps and deeper diagnostics, see:

- `docs/manual-test-account-setup.md`
- `docs/manual-test-collection-backend.md`
- `docs/manual-test-ui-features.md`
