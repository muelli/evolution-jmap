<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Fastmail OAuth 2.0 — does the autodiscovery-only design work? (research, 2026-08-19)

**Question (operator-requested):** does Fastmail support OAuth 2.0 *dynamic
client registration* (RFC 7591)? The project's OAuth design is
autodiscovery-only (decision #1 in `ROADMAP.md`): discover the provider's
endpoints, dynamically register a public client, run authorization-code + PKCE
— with **no** shipped/pre-registered `client_id`. That only works if the
provider offers open dynamic registration.

**Answer: yes.** Fastmail's live RFC 8414 authorization-server metadata
advertises a `registration_endpoint`, so the autodiscovery-only design is
viable against it.

## Evidence — the live metadata, fetched verbatim

`GET https://api.fastmail.com/.well-known/oauth-authorization-server`:

```json
{
    "issuer": "https://api.fastmail.com",
    "registration_endpoint": "https://api.fastmail.com/oauth/register",
    "authorization_endpoint": "https://api.fastmail.com/oauth/authorize",
    "token_endpoint": "https://api.fastmail.com/oauth/refresh",
    "scopes_supported": [
        "urn:ietf:params:oauth:scope:mail",
        "urn:ietf:params:oauth:scope:contacts",
        "urn:ietf:params:oauth:scope:calendars",
        "https://www.fastmail.com/dev/mcp",
        "openid", "profile", "email", "offline_access"
    ],
    "response_types_supported": ["code"],
    "grant_types_supported": ["authorization_code", "refresh_token"],
    "token_endpoint_auth_methods_supported": ["none"],
    "code_challenge_methods_supported": ["S256"],
    "authorization_response_iss_parameter_supported": true,
    "revocation_endpoint": "https://api.fastmail.com/oauth/revoke",
    "revocation_endpoint_auth_methods_supported": ["none"]
}
```

`registration_endpoint` present (RFC 8414 §2 defines it as the RFC 7591
endpoint) + `token_endpoint_auth_methods_supported: ["none"]` (public clients,
no secret) + `code_challenge_methods_supported: ["S256"]` (PKCE) is exactly the
public-client + DCR + PKCE model the design assumes.

## One discrepancy, resolved

Fastmail's *human* doc (`https://www.fastmail.com/for-developers/oauth/`) still
says clients are "registered manually by contact with Fastmail developers."
That contradicts the live metadata. Reading: **the doc lags the deployment** —
dynamic registration looks recently added (note the `https://www.fastmail.com/dev/mcp`
scope; the MCP OAuth profile mandates DCR). The machine-readable metadata is
what an RFC 8414 client actually reads at runtime and is the ground truth for
what the server will accept.

**CONFIRMED 2026-08-19 — registration is open, empirically, not merely
inferred from the metadata.** This runner has ordinary internet egress to
`api.fastmail.com` (a public RFC 8414/7591 endpoint, no different from what
any real client's autodiscovery does — no operator credentials or session
involved). A bare, unauthenticated `POST /oauth/register` with just
`client_name`/`redirect_uris` returned **HTTP 201** with a fresh `client_id`,
no initial access token needed:

```
$ curl -s -X POST https://api.fastmail.com/oauth/register -H 'Content-Type: application/json' \
    -d '{"client_name":"...","redirect_uris":["org.gnome.evolution.jmap:/redirect"]}'
{"client_id_issued_at":...,"scope":"","response_types":["code"],"token_endpoint_auth_method":"none",
 "client_id":"673641ae","...","grant_types":["authorization_code","refresh_token"],"redirect_uris":[...]}
```

**A second, more consequential finding from the same probe: the registered
default `scope` is empty when the request names none.** Per RFC 6749 §3.3, an
authorization request that itself omits `scope` may fall back to exactly this
per-client default — so a client that registers with no scope (as this
project's did, before this session) risks a token with **no JMAP access at
all**, silently. Repeating the registration with an explicit
`"scope": "urn:ietf:params:oauth:scope:mail urn:ietf:params:oauth:scope:contacts urn:ietf:params:oauth:scope:calendars"`
had the server record and echo back exactly that string as the client's
scope — confirming the server treats a named scope at registration time as
the client's default. **Fixed the same session:** `jmap_client::oauth::
ClientRegistrationRequest` gained a `scope: Option<&str>` field;
`jmap_config::oauth2_setup::discover_and_register` now passes the discovered
`scopes_supported` joined with a space when the deployment names any, and
`None` (omitting the field entirely, unchanged from before) when it names
none. This closes the "confirm
`/oauth/register` is open" item in `ROADMAP.md`'s CURRENT PRIORITY 2(b) and
the scope-naming item together.

## Concrete constraints for our implementation (found pre-implementation)

- **Redirect URI — our current value would be REJECTED.**
  `config_lookup::REDIRECT_URI = "jmap-oauth2:/redirect"` has no dot. Fastmail's
  doc requires a private-use scheme in **reverse-DNS notation with at least one
  dot** (e.g. `com.example:/`), *or* loopback `http://localhost/` (arbitrary
  port, `127.0.0.1`/`::1` allowed), *or* an owned `https` domain. EDS's
  `ECredentialsPrompterImplOAuth2` uses an embedded WebKitView (not a loopback
  server) and extracts `code=` from whatever URI the navigation finishes on, so
  a private-use scheme is the right shape — but it must be **dotted**
  reverse-DNS (e.g. `org.gnome.evolution.jmap:/redirect` or similar). Change it.
- ~~**Scopes:** use the metadata's `urn:ietf:params:oauth:scope:{mail,contacts,calendars}`,
  not the older `urn:ietf:params:jmap:mail` the stale human doc shows.~~ **Fixed
  2026-08-19** — `discover_and_register` now registers with every scope the
  deployment's own metadata advertises (`scopes_supported`, joined), so this
  is handled generically rather than by naming Fastmail's specific strings; see
  above.
- **Token endpoint** is `https://api.fastmail.com/oauth/refresh` (non-obvious
  name). Fine *because* we discover it via RFC 8414 — never hardcode `/token`.
- **PKCE `S256` is mandatory**; the AS advertises only S256.
- **RFC 8707 `resource` indicator:** absent from both the metadata and the doc.
  One web-search summary claimed Fastmail's `/authorize` rejects requests
  lacking a `resource` = the JMAP session URL; unconfirmed against a primary
  source. Verify empirically whether authorize/token need it.
  **PARTIALLY CONFIRMED 2026-08-19 — the `/authorize` half, empirically, not
  merely inferred.** This runner has the same ordinary internet egress to
  `api.fastmail.com` the registration probes above used. Registered one more
  throwaway public client (`POST /oauth/register`, same shape as above), then
  `GET /oauth/authorize` with `client_id`, `redirect_uri`, `response_type=code`,
  a PKCE `code_challenge`/`code_challenge_method=S256`, and `scope` — but
  **no** `resource` parameter at all:

  ```
  $ curl -s -D - -o /dev/null -G https://api.fastmail.com/oauth/authorize \
      --data-urlencode "client_id=66de41ae" \
      --data-urlencode "redirect_uri=org.gnome.evolution.jmap:/redirect" \
      --data-urlencode "response_type=code" \
      --data-urlencode "code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM" \
      --data-urlencode "code_challenge_method=S256" \
      --data-urlencode "scope=urn:ietf:params:oauth:scope:mail ..."
  HTTP/2 302
  location: https://app.fastmail.com/oauth/?client_id=66de41ae&redirect_uri=...&response_type=code&code_challenge=...&code_challenge_method=S256&scope=...&state=...
  ```

  No rejection, no error redirect naming `invalid_request`/`resource` — a
  plain `302` handing the request off to the login/consent UI at
  `app.fastmail.com`, request parameters echoed back verbatim. Reproduced
  twice with two different freshly-registered `client_id`s, same result both
  times. This **refutes** the unconfirmed web-search claim above: Fastmail's
  authorization endpoint does not require (or even notice the absence of) a
  `resource` parameter at this stage. **Still open, and still needs the
  operator:** whether the **token** endpoint (`/oauth/refresh`) cares once a
  real authorization code exists — that needs an actual consent round-trip
  (real credentials, real login), which is inherently a human step this
  runner cannot fake. No request in this probe was completed past the
  redirect — nothing was submitted to the login page, no account was touched,
  and the two probe clients registered here are throwaway (unusable without
  a code obtained through that login page).

## Prerequisite already met

Reaching `api.fastmail.com` at all needs the RFC 8620 SRV autodiscovery, which
the night agents implemented after this was queued (Resolver seam in
`jmap-client` — `a07f1a6`; both call sites routed through it — `2881ac5`,
`bdca950`). So the OAuth-via-Look-Up path is the next real-server step, and it
is still **DEFERRED** pending a TLS-proper deployment + a human running the
consent round-trip (see `ROADMAP.md`). This document is the input to that work.

## Sources

- Fastmail authorization-server metadata (primary): <https://api.fastmail.com/.well-known/oauth-authorization-server>
- Fastmail OAuth developer doc: <https://www.fastmail.com/for-developers/oauth/>
- Fastmail API overview: <https://www.fastmail.com/dev/>
- RFC 7591 (Dynamic Client Registration): <https://www.rfc-editor.org/rfc/rfc7591>
- RFC 8414 (Authorization Server Metadata): <https://www.rfc-editor.org/rfc/rfc8414>
