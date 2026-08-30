<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# DRAFT, not filed

Target: `GNOME/evolution-data-server`, issue. Written 2026-08-29 against EDS
3.52.3 while answering `docs/ROADMAP.md` item 26. Filed only after a
maintainer has read it, same rule as the two Evolution drafts beside this
file.

---

## `Failed to lookup password for source <uid>` names a source whose secret was never looked up

### What happens

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
`e_source_credentials_provider_lookup` resolves the credentials source first,
`source_credential_provider_ref_impl_for_source`
(`src/libedataserver/e-source-credentials-provider.c:216-237`) calls
`e_source_credentials_provider_ref_credentials_source`, which walks up the
`Parent=` chain to the nearest source carrying `[Collection]` (line 416-437),
because "the credentials are usually stored on the collection source, thus
shared between child sources".

So for every child of every collection account, which is to say for most
accounts anybody has, the message names the child while
`e_source_credentials_provider_impl_password_lookup_sync` searched under the
collection's UID (`e_source_lookup_password_sync`, `e-source.c:4215-4221`).

### Why it matters

The UID in that message is the only identifier an operator gets, and it invites
exactly the wrong investigation. In our case it sent two sessions looking for
debris:

* the UID is a collection *child*, so it has no keyfile in
  `~/.config/evolution/sources/`; children live in
  `$XDG_CACHE_HOME/evolution/sources/<collection-uid>/`
  (`collection_backend_new_user_file`, `e-collection-backend.c:176-200`). It
  therefore reads as "a UID that matches nothing I configured".
* and grepping the keyring for it finds nothing either, correctly, because
  the entry that is missing is the *collection's*.

Both dead ends disappear if the message names the source that was actually
searched.

### Suggested change

Report both when they differ. `e_source_credentials_provider_lookup` already
computes the credentials source; passing it into `ReinvokeCredentialsRequiredData`
(or re-resolving it in the callback, which is cheap) would allow:

```
Failed to lookup password for source <child-uid> (<child-name>);
credentials are held by <collection-uid> (<collection-name>): <error>
```

### Reproduction

Headless, no interaction, ~8 seconds:
`rust/crates/jmap-functional/tests/stale-source-uid.rs` in
<https://github.com/…/evolution-jmap> stands up a real registry over
`dbus-run-session`, fans one collection account out into children, and asserts
the child UID in the log line differs from the credentials-source UID. Its
sibling test asserts the other half: that storing the password under the
collection's UID makes the same request succeed silently.
