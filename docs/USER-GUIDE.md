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

### Server filters

Open the account settings. Look for the `Server Filters` page of the JMAP account.

This feature needs server support. If the server does not offer it, the page shows a message and stays empty.

The top list shows every filter script on the server and which one is active. Use `Add`, `Rename`, `Remove`, `Activate` and `Deactivate` to manage scripts.

Most users only need the rules list below it. It manages one script named `evolution-jmap`:

- Use `Add` to create a rule. Pick a condition, such as `Subject contains`, and an action, such as `Move to`.
- Use `Edit` to change a rule. Use `Remove` to delete it.
- Use `Move Up` and `Move Down` to change the order rules run in.
- Use `Enable` and `Disable` to turn a rule on or off without deleting it.

Turn on `Edit as text` to write the script by hand instead of using the rules list. If a script was changed outside Evolution and no longer matches the rules list, the page shows it as text until it matches again.

Use `Check` to test a script on the server. `Check` never saves the script.

Use `Edit in External Editor` to edit the script text in another program. Evolution picks the program in this order: the External Editor plugin's own command, then `$VISUAL`, then `$EDITOR`. If none of these is set, Evolution shows a message and does not open the script. It does not fall back to a desktop default handler for text, because Evolution cannot then tell when you are done editing.

`Activate`, `Deactivate`, `Rename`, `Add` and `Remove` take effect at once. You do not need to click `OK` in the account editor for these. `Activate` first checks the script and refuses a broken one. A script's text saves on its own soon after you stop typing, once it parses. `OK` still saves anything not yet sent; `Cancel` cannot undo a change already sent.

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

### Shared mailboxes

A server can grant you access to another user's mailboxes. Evolution shows them as one extra branch per sharing user, under your own mail account, the same way it shows another user's folders over IMAP.

Evolution shows every mailbox the server grants, even one the owner has not marked as subscribed. A grant you have not subscribed to still appears.

Your rights in each shared mailbox can differ from your rights in your own account. Evolution hides an action a mailbox's grant does not allow, such as Create, Delete, Rename or changing a star, label or category.

A read mark (`$seen`) on a shared message is shared state on the message, not a mark kept per person. When you or the owner reads a message in a shared mailbox, every other person with access sees it as read too.

A share does not carry over to a child mailbox you create inside it. It grants only the mailbox itself. A share also does not let you send mail as the owner.

## Stars, labels and categories

Use the star to flag a message. Use a label to put a message in a category, such as Work or Personal. Evolution offers five built-in labels: Important, Work, Personal, To Do, and Later. You can also add a label with your own name.

Events and contacts have categories instead of labels. Add, rename or clear a category in the event editor or the contact editor.

The server stores a star, a label and a category as a keyword on the message, event or contact. A keyword name is not case sensitive: `Work` and `work` are the same keyword.

### What other mail clients show

Other JMAP and IMAP clients write some keywords under their own names. Evolution keeps every keyword it does not recognize, so nothing is lost when you change one flag or label on a message:

- Thunderbird writes a label as `$label1` to `$label5`. Evolution shows each as a label with that literal name, not as one of its own five.
- Apple Mail writes a colored flag as `$flagged` plus one of `$mailflagbit0` to `$mailflagbit2`. Evolution shows the star and keeps the color bits as labels.
- Fastmail writes a pinned message as `$pinned`. Evolution keeps it as a label.
- Fastmail can also show a label as a folder. A message with such a label then appears in more than one folder. Evolution does not change this when you flag or label the message differently.
- Roundcube writes junk status as `junk` and `nonjunk` tags next to `$junk` and `$notjunk`. Evolution keeps all four.

## Working offline

A JMAP account works offline much like any other Evolution account.

To work offline on purpose, open `File`, then `Work Offline`. Evolution also starts offline on its own when it finds no network.

### What you can read offline

- The folder list and the message list from your last sync.
- A message you already opened.
- A folder you downloaded with `Download messages for offline use` or `Copy folder content locally`.
- Contacts and calendar events you already loaded.

### What you can still do, and Evolution sends it later

These actions work offline. Evolution queues each one and sends it to the server once you go back online. You do not need to repeat the action yourself.

- Mark a message read or unread. Flag it. Mark it junk or not junk. Add or remove a label.
- Move or copy a message to another folder.
- Delete a message, or empty the trash.
- Save a draft.
- Create, edit or delete a contact or a calendar event.

A move, a copy and a delete show in the message list right away. You do not need to wait for the next sync to see them.

### If a queued change is refused

Most servers accept a queued change once you go back online. A few refusals are final: for example, you lost the right to change that folder, or the folder no longer exists.

For a final refusal, Evolution undoes the change in the message list and reports the server's reason as a mail error, the same way it reports any other folder error. A move or a copy then shows the message back where it was before you moved or copied it.

A refusal that is not final, such as a full quota or a server that is briefly unreachable, keeps the change queued. Evolution tries again at the next sync.

### What needs a connection right away

A few actions need the server at once. Evolution shows an "offline" message or an error instead of queuing these:

- Accepting or declining a meeting invitation.
- Saving the `Server Filters` or `Vacation Responder` page. Each page shows "You are offline" when you open it offline, instead of failing only when you save.
- Scheduling a message with `Send Later`. The menu shows "You are offline" and stays disabled.

## Known limits

- Snooze works only on servers that offer the Cyrus snooze capability.
- If the server advertises an `eventSourceUrl`, mail uses JMAP Push.
  If it does not, new mail arrives at the refresh interval of the account.
- Checking another person's free or busy time does not yet fall back to locally cached calendar data. It can show no information, online or offline.

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
