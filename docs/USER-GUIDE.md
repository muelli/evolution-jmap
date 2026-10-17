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

`gh attestation verify` currently needs GitHub CLI authentication (`gh auth login` or
`GH_TOKEN`). If you do not have that, use the no-token fallback in
`docs/verifying-artifacts.md`.

Fedora (`.rpm` build or release asset):

```bash
sudo dnf install ./evolution-jmap*.rpm
evolution --force-shutdown
```

Unverified in a running Evolution: exact package filename pattern on every distro release.

## Add a Fastmail account (OAuth)

Use this flow for Fastmail and similar providers that support OAuth 2.0.

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

If lookup does not offer OAuth for that server, Evolution keeps `Authentication`
on `Password`, and you can finish with password or API token authentication instead.

Unverified in a running Evolution: exact button labels for the OAuth lookup and sign-in steps across Evolution versions.

## Add a Stalwart account (password)

Use this flow for Stalwart and other password-based setups.

1. Open Evolution.
2. Start adding a new mail account.
3. Select the account type named `JMAP`.
4. Fill in:
   - `Server`: your Stalwart host or IP (for example `<stalwart-ip>`)
   - `Port`: `443` for HTTPS, or your custom port
   - `Username`: your Stalwart login
   - `Authentication`: `Password`
   - `Use a secure connection (TLS)`: enabled for HTTPS; disable only for local/plain HTTP testing
5. Continue and save the account.

The account setup module writes these settings into the account source:

- `[Authentication] Host` from `Server`
- `[Authentication] User` from `Username`

These values are consumed by the collection backend and the generated mail/calendar/address-book sources.

## What you should see after setup

A working JMAP account should appear as one account with three service areas:

- mail folders and send/receive
- contacts/address books
- calendars

The project manual test recipe confirms this end-to-end against real servers and the local mock server.

## Token rejected: what to expect

If an OAuth token is revoked or otherwise rejected, Evolution asks you to sign in again.

Mail uses separate receive and send services, so a rejected token can trigger one consent prompt per service. This is expected from EDS authentication flow, not a duplicate-account bug.

If prompts loop forever without a successful sign-in, treat it as an error and capture debug logs.

## Trailing-dot host case

Hosts with one trailing dot are normalized to the same host without the dot. Example: `api.fastmail.com.` is treated as `api.fastmail.com`.

No account rewrite is required on current builds. Normalization happens during connection target resolution.

## JMAP-only features

### Vacation autoresponder

Open account settings and look for a `Vacation Responder` page for the JMAP account. You can enable automatic replies, set optional start/end dates, subject, and message.

Unverified in a running Evolution: exact widget layout on every desktop theme and Evolution release.

### Scheduled send

In the composer, open `File` then `Send Later`.

This menu is enabled only when the selected From account is JMAP and the server advertises delayed-send support (`maxDelayedSend > 0`).

Current presets are:

- `In One Hour`
- next local `08:00`, `12:00`, `18:00`, and `21:00` (shown as Today, Tomorrow, or weekday)
- next workday at `09:00` (Monday to Friday)

### Snooze

In a message list context menu, use `Snooze` presets.

Important limit: many public JMAP servers do not expose the snooze capability to third-party clients. In that case the menu stays disabled for that account. This is expected behavior.

## Known limits

- Evolution does not have an EDS hook to surface JMAP push (`eventSourceUrl`) in this plugin architecture. The provider runs pull-based refresh instead.
- Snooze is capability-gated. Many public servers currently do not advertise the capability for API clients.
- Some UI details in this guide are marked unverified in a running Evolution where they depend on desktop/runtime specifics.

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

- Recheck `Server`, `Port`, and `Username` first.
- If mail authentication fails, confirm account credentials and whether OAuth or password flow is configured for that provider.
- If `Send Later` is disabled, verify the active From account is JMAP and the server supports delayed send.
- If `Snooze` is disabled, verify whether your server advertises snooze capability. Lack of capability is common and not necessarily a bug.

For developer-oriented validation steps and deeper diagnostics, see:

- `docs/manual-test-account-setup.md`
- `docs/manual-test-collection-backend.md`
- `docs/OAUTH-FASTMAIL.md`
- `docs/manual-test-ui-features.md`
