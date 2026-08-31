<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Submit-ready. File at https://gitlab.gnome.org/GNOME/evolution-data-server/-/issues

Everything below the horizontal rule is the issue body.

---

## Title

Two accounts authenticating as the same user share one OAuth2 secret-store
slot when they are on different servers

## Summary

[`eos_generate_secret_uid()`](https://gitlab.gnome.org/GNOME/evolution-data-server/-/blob/3.52.3/src/libedataserver/e-oauth2-service.c#L1086-L1112)
derives the secret-store key an `ESource`'s OAuth 2.0 tokens are stored and
looked up under as:

```c
*out_uid = g_strdup_printf ("OAuth2::%s[%s]", e_oauth2_service_get_name (service), user);
```

`service` is the registered `EOAuth2Service`'s name (`"Google"`, `"Outlook"`,
`"JMAP"`, ...) and `user` is `[Authentication] User`. **The host is not part
of the key.**

Every in-tree `EOAuth2Service` implementation (Google, Outlook/Office365,
Yahoo) talks to exactly one cloud, so `(service name, user)` is unique for
those by construction — there is only ever one server a given service name
could mean. That assumption does not hold for a **multi-server** protocol:
an `EOAuth2Service` for IMAP/JMAP/CalDAV-over-a-self-hostable-protocol can
be registered once and used against any number of independent deployments,
each with its own client id, its own token endpoint, and its own idea of who
`alice@example.com` is. Two accounts using that one service, authenticating
as the same address on two different servers, derive the identical secret
key and silently share one token slot.

## Affected versions

Observed on evolution-data-server 3.52.3 (Ubuntu package
`3.52.3-0ubuntu1.2`). `eos_generate_secret_uid()` is unchanged on `main` as
of this writing — same format string, same two inputs, no host anywhere in
the function — so this is not fixed.

## Reproduction

Reproduced headlessly against two mock JMAP deployments (standing in for two
independent real servers) using an out-of-tree `EOAuth2Service`
(`evolution-jmap`, https://github.com/muelli/evolution-jmap, whose
`[Authentication] Method=JMAP` names one `EOAuth2Service` shared by every
account regardless of which JMAP server it talks to — the exact multi-server
shape this bug requires):

1. Account A and account B both carry `[Authentication] User=someone@example.com`,
   but a different `Host`, `Port` and `[JMAP OAuth2] TokenEndpoint` each —
   two unrelated deployments.
2. Account A is seeded with a refresh token and asked for an access token.
   It performs a real refresh grant against **its own** token endpoint and
   receives a fresh, long-lived access token, which
   `eos_store_token_sync()` files under `OAuth2::JMAP[someone@example.com]`.
3. Account B, which has never been seeded and never consented to anything,
   is then asked for an access token of its own.
4. Account B's token endpoint is asked nothing at all. Account B's fetch
   succeeds anyway, answered entirely out of the shared secret-store slot —
   and the token it receives is not merely *a* valid-looking string, it is
   byte-for-byte the access token minted for account A.

Full, runnable test: `rust/crates/jmap-functional/tests/oauth2-token-collision.rs`
and its client `tests/functional/oauth2-token-collision-client.c`, both in
the repository above (`ctest -L functional -R oauth2-token-collision`).

The reproduction deliberately does not use two real servers. The defect is
entirely inside `eos_generate_secret_uid()`'s own key derivation and does not
depend on what either server answers, so two independently configured mock
deployments reproduce it exactly and, unlike a repro tied to disposable
infrastructure, can be run by anyone who checks the linked commit out.

## Impact

For the protocol this was found through: an account at one deployment (say,
a hosted provider) and an account at a second, unrelated deployment (say, a
self-hosted server), both configured with the same email address, silently
share one secret-store entry. What actually happens next depends on refresh
timing at each account's own server, but every path is a correctness
failure:

- Whichever account refreshes first fills the shared slot with its own
  tokens. The other account then either uses that account's live access
  token against its own, different server (until that server notices the
  bearer token belongs to nobody it knows and refuses it) — or, if the
  token happens to still work at the byte level but not at the account
  level, silently operates as the wrong identity.
- When a refresh *is* needed and the stored refresh token belongs to the
  other account, the request goes to the right token endpoint with the
  wrong refresh token (issued by a different deployment entirely), which
  fails; the client falls back to an interactive consent window for an
  account whose stored credentials were, from the user's point of view,
  perfectly fine a moment ago.
- Consenting again in that window stores a fresh token pair under the same
  shared key, in turn clobbering whatever the first account had — a
  ping-pong between the two accounts' consent windows that has no stable
  resolution as long as both remain configured with the same address.

None of this is specific to one client implementation: it follows directly
from the key derivation, for any `EOAuth2Service` whose accounts are not all
the same single server.

## Suggested fix

Include the host (or another value that distinguishes deployments, e.g. the
account's own `ESource` UID) in the key `eos_generate_secret_uid()` builds —
something like:

```c
*out_uid = g_strdup_printf ("OAuth2::%s[%s@%s]", e_oauth2_service_get_name (service), user, host);
```

This needs a migration path for slots that already exist under the current,
host-blind key, since a live upgrade must not orphan every already-consented
account's stored tokens.

## Related reports

This is the fourth report from the same client-side investigation as
https://gitlab.gnome.org/GNOME/evolution-data-server/-/issues/661 (also a
single-provider assumption baked into shared EDS machinery, there in a
different subsystem). Not a duplicate: #661 is about a different assumption
in a different code path.
