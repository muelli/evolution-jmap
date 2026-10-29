<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# User guide: adding and using a JMAP account in Evolution

This guide tells end users how to use JMAP with Evolution.

## Install the package

Install one package format. Then restart Evolution so that it loads the modules again.

Debian or Ubuntu (`.deb` release asset):

```bash
sha256sum --check --ignore-missing SHA256SUMS
gh attestation verify evolution-jmap_*_$(dpkg --print-architecture).deb --repo muelli/evolution-jmap
sudo apt install ./evolution-jmap_*_$(dpkg --print-architecture).deb
evolution --force-shutdown
```

`gh attestation verify` needs GitHub CLI authentication (`gh auth login` or
`GH_TOKEN`). If you have no authentication, use the cosign steps in
`docs/verifying-artifacts.md`.

Fedora: releases do not include an `.rpm`. Build one with `ci/rpm.sh` (see
`docs/PACKAGING.md`). Then do this:

```bash
sudo dnf install ./build-rpm/*.rpm
evolution --force-shutdown
```

## Add a Fastmail account (OAuth)

Use this procedure for Fastmail and for other servers that support OAuth 2.0.
You do not need a pre-registered client. The module registers itself with the
server through OAuth dynamic client registration (RFC 7591), which Fastmail
offers. You can set up OAuth 2.0 only through the lookup in this procedure.
The manual server settings page does not support it.

1. Open Evolution and add a new mail account.
2. On the identity page, enter your Fastmail address.
3. Click `Look Up Account Details`.
4. Pick the `JMAP` result.
5. On `Receiving Email`, make sure that the settings are as follows:
   - `Server`: `api.fastmail.com`
   - `Port`: `443` (or blank, which means the default HTTPS port)
   - `Username`: your Fastmail address
   - `Authentication`: `OAuth 2.0`
   - `Use a secure connection (TLS)`: enabled
6. Sign in and give consent in the browser.
7. Return to Evolution and finish the setup.

If the lookup does not offer OAuth for the server, use `Password` or
`API Token` authentication.

## Add a Stalwart account (password)

Use this procedure for Stalwart and for other servers that use password or
API token sign-in.

1. Open Evolution.
2. Start to add a new mail account.
3. Select the account type `JMAP`.
4. Enter these settings:
   - `Server`: the address of your Stalwart server (for example `<stalwart-ip>`)
   - `Port`: `443` for HTTPS, or your custom port
   - `Username`: your Stalwart login
   - `Authentication`: `Password` (or `API Token`)
   - `Use a secure connection (TLS)`: enabled. The module accepts plain HTTP
     only for a loopback address (`127.0.0.1`, for local tests).
5. Continue and save the account.

The account setup module writes these settings into the account source:

- `[Authentication] Host` from `Server`
- `[Authentication] User` from `Username`

The collection backend reads these settings. It uses them to create the mail,
calendar and address book sources.

## What you see after setup

A working JMAP account appears as one account with three service areas:

- mail folders and send/receive
- contacts (address books)
- calendars

## Token rejected: what to expect

If the server revokes or rejects an OAuth token, Evolution asks you to sign in again.

Mail uses separate receive and send services. A rejected token can therefore
cause one consent prompt for each service. This is the normal behavior of the
EDS authentication flow. It is not a duplicate-account bug.

If the prompts repeat and sign-in never succeeds, treat this as an error. Capture the debug logs.

## Trailing-dot server names

The module removes one trailing dot from a server name. For example, it treats `api.fastmail.com.` as `api.fastmail.com`.

You do not need to change existing accounts.

## JMAP-only features

### Vacation autoresponder

Open the account settings. Look for the `Vacation Responder` page of the JMAP account. On this page, you can enable automatic replies. You can also set optional start and end dates, a subject and a message.

### Scheduled send

In the composer, open `File`, then `Send Later`. The menu offers these presets:

- `In One Hour`
- The next local `08:00`, `12:00`, `18:00` and `21:00`. The menu labels each one Today, Tomorrow or a weekday name.
- The next workday (Monday to Friday) at `09:00`

The menu is enabled only if both conditions are true:

- The selected From account is a JMAP account.
- The server advertises delayed-send support (`maxDelayedSend > 0`).

### Snooze

In the context menu of a message list, use the `Snooze` presets.

Snooze needs the Cyrus snooze capability. Stalwart and Fastmail do not offer it. The menu therefore stays disabled for their accounts. This is the expected behavior.

### Externally hosted contact photos

A contact can keep its photo on a web server instead of in the contact. The address book preview then shows the text `Externally hosted image` and the full address of the image. The preview does not download the image.

This keeps the owner of that web server from learning when you look at the contact.

## Known limits

- Snooze works only on servers that offer the Cyrus snooze capability.
- If the server advertises an `eventSourceUrl`, mail uses JMAP Push.
  If it does not, new mail arrives at the refresh interval of the account.

## Logs and debug output

- Runtime logs go to journald when it is available. Otherwise they go to stderr.
- Each Evolution process reads `EVOLUTION_JMAP_LOG` when it starts. If Evolution is running, close it first.
- To get detailed module logs, restart Evolution with this command:
  `EVOLUTION_JMAP_LOG=debug G_MESSAGES_DEBUG=all evolution`
- To watch the live user-session logs, run `journalctl --user -f`.
- At the `trace` level, the log contains the body of each JMAP request and response. These bodies contain your mail, contacts and calendar events: subjects, addresses, message previews, attachment names and the text of contacts and events. The log does not contain your password or your tokens.
- Use `trace` only to find a problem. Set the level back to `debug` or lower when you are done.
- Read a `trace` log before you attach it to a bug report. Remove the private data, or send only the lines that show the problem.
- If journald is not available (for example in some containers), start Evolution from a terminal. Read stderr in that terminal.
- The account source files are in `${XDG_CONFIG_HOME:-~/.config}/evolution/sources/`.

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
- If mail authentication fails, check the account credentials. Also check whether the provider uses OAuth or password authentication.
- If `Send Later` is disabled, make sure that the active From account is a JMAP account. Also make sure that the server supports delayed send.
- If `Snooze` is disabled, check whether your server advertises the snooze capability. Many servers do not. This is not necessarily a bug.

For developer test steps and more diagnostics, see:

- `docs/manual-test-account-setup.md`
- `docs/manual-test-collection-backend.md`
- `docs/manual-test-ui-features.md`
