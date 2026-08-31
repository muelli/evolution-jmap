<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# READY TO FILE (reviewed 2026-08-31)

Target: `GNOME/evolution-data-server`, issue. Paste the title and everything
below the rule as the issue body. Validated 2026-08-30/31: the child-under-
collection layout was confirmed on the test VM, the cited EDS code paths were
re-read, and no duplicate exists upstream (nearest is GNOME/evolution-data-server#29,
a different bug). Scope: this reports ONLY the misleading log message; the
lookup semantics themselves are correct and no functional failure is claimed.

Suggested title:
> `Failed to lookup password for source <uid>` names a source whose secret was never looked up

---
Found against EDS 3.52.3 while debugging a collection account.

## What happens

With `ESR_DEBUG=1`, a failing credentials lookup in `evolution-source-registry`
logs (`src/libebackend/e-server-side-source.c:459-465`):

```c
printf ("%s: Failed to lookup password for source %s (%s): %s\n", G_STRFUNC,
        e_source_get_uid (E_SOURCE (data->source)),
        e_source_get_display_name (E_SOURCE (data->source)),
        error ? error->message : "Unknown error");
```

`data->source` is the source the `InvokeCredentialsRequired` call was *about*.
It is not, in general, the source the secret store was searched under.

`e_source_credentials_provider_lookup` resolves the credentials source first:
`source_credential_provider_ref_impl_for_source`
(`src/libedataserver/e-source-credentials-provider.c:216-237`) calls
`e_source_credentials_provider_ref_credentials_source`, which walks up the
`Parent=` chain to the nearest source carrying `[Collection]` (lines 416-437),
because as the comment there says, the credentials are usually stored on the
collection source and shared between child sources.

So for every child of every collection account, which is to say for most
accounts anybody has, the message names the child while
`e_source_credentials_provider_impl_password_lookup_sync` searched under the
collection's UID (`e_source_lookup_password_sync`, `e-source.c:4215-4221`).

## Why it matters

The UID in that message is the only identifier an operator gets, and it invites
exactly the wrong investigation. In our case it cost two debugging sessions:

* The UID is a collection *child*, so it has no keyfile in
  `~/.config/evolution/sources/`. Children live in
  `$XDG_CACHE_HOME/evolution/sources/<collection-uid>/`
  (`collection_backend_new_user_file`, `e-collection-backend.c:176-200`).
  It therefore reads as "a UID that matches nothing I have configured", and
  looks like leftover debris from a deleted account.
* Grepping the keyring for that UID also finds nothing, correctly, because the
  entry that is actually missing belongs to the *collection*.

Both dead ends disappear if the message names the source that was searched.

## Suggested change

Report both when they differ. `e_source_credentials_provider_lookup` already
computes the credentials source, so passing it into
`ReinvokeCredentialsRequiredData` (or re-resolving it in the callback, which is
cheap) would allow:

```
Failed to lookup password for source <child-uid> (<child-name>);
credentials are held by <collection-uid> (<collection-name>): <error>
```

## Reproduction

Headless, no interaction, about 8 seconds:
https://github.com/muelli/evolution-jmap/blob/master/rust/crates/jmap-functional/tests/stale-source-uid.rs

It stands up a real registry over `dbus-run-session`, fans one collection
account out into children, and asserts that the child UID in the log line
differs from the credentials-source UID. Its sibling test asserts the other
half: storing the password under the collection's UID makes the same request
succeed silently.

## Note on scope

This report is about the diagnostic message naming the wrong source. Whether a
given lookup *should* have succeeded is a separate question and is not claimed
here.
