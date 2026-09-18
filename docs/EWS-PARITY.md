# EWS parity audit

A systematic, surface-by-surface diff between this project's
Evolution-facing integration layer and evolution-ews's, the mature reference
implementation of the same kind of plugin. Motivation: three of the five
bugs found in the first live OAuth run were exactly the kind of
divergence-from-the-template a diff like this would have caught proactively
(missing `auto_configure`, a lookup-result priority tie, per-process
`EOAuth2Service` registration). This document is that diff, done once
broadly rather than bug-by-bug.

evolution-ews source is read from `gitlab.gnome.org/GNOME/evolution-ews`,
`master` branch (no `gnome-3-52` branch exists on that repo; `master` is the
closest available and the vfunc/registration shapes audited here have not
moved across GNOME 3.52-era Evolution/EDS). All file:line references into
this project are relative to `rust/crates/`.

Out of scope, per the roadmap item's own text: the OAuth2 **acquisition**
flow (RFC 8414 discovery, RFC 7591 dynamic registration, scope selection,
token refresh) — EWS hardcodes a static single-provider (Office 365) client
there, so there is no template to diff against; that layer already has its
own tests and its own roadmap items (5, 6, 12).

## Surface 1 — Module registration points per process

The one surface audited directly (not delegated), because it already has a
3-for-3 hit rate finding real bugs this thread (`f83e04b`, item 12) and is
concretely bounded: enumerate every GType each project registers, in which
module, loaded by which process.

evolution-ews's `EOAuth2ServiceOffice365`, read from its actual source
(`src/EWS/registry/module-ews-backend.c`,
`src/EWS/evolution/module-ews-configuration.c`,
`src/EWS/addressbook/e-book-backend-ews-factory.c`,
`src/EWS/calendar/e-cal-backend-ews-factory.c`):

| Process | Module | Registers `EOAuth2ServiceOffice365`? |
|---|---|---|
| Evolution shell | `module-ews-configuration.c` | yes |
| `evolution-source-registry` | `module-ews-backend.c` | yes |
| `evolution-addressbook-factory` | `e-book-backend-ews-factory.c` | yes |
| `evolution-calendar-factory` | `e-cal-backend-ews-factory.c` | yes |
| Camel (mail) | (uses `camel_sasl_xoauth2_office365` instead — a different, wire-level SASL mechanism, registered in the shell module above, not a per-process `EOAuth2Services` lookup) | n/a |

This project's `jmap_config::oauth2_service::Service`, after item 12's fix
(`0f4e...`/session N+55, this session's own predecessor):

| Process | Module | Registers `Service`? |
|---|---|---|
| Evolution shell | `jmap-config/src/module.rs` | yes (`f83e04b`) |
| `evolution-source-registry` | `jmap-backend-collection/src/module.rs` | yes |
| `evolution-addressbook-factory` | `jmap-backend-book/src/module.rs` | yes (item 12) |
| `evolution-calendar-factory` | `jmap-backend-cal/src/module.rs` | yes (item 12) |
| Camel (mail) | (uses `camel_session_get_oauth2_access_token_sync` directly, no registered `EOAuth2Service` needed — `jmap-mail/src/oauth2.rs`) | n/a |

**Verdict: exact parity, four-for-four**, now that item 12 landed. This also
resolves a standing inaccuracy in this project's own documentation, found and
fixed as part of this audit (not merely noted): both
`jmap-backend-collection/src/module.rs`'s "why the OAuth2 service registers
here too" doc comment and `jmap-backend-collection/tests/oauth2_service.rs`'s
module doc claimed evolution-ews registers its OAuth2 service "in exactly
this module... and nowhere else" — a claim that was simply **wrong**, not
merely incomplete, going by the four-module table above. That wrong claim is
exactly what argued (before item 12's operator-found bug forced a correction)
that registering only in the registry process would be enough for this
project too. Both comments are corrected in this session to name the real
evolution-ews shape, with a pointer back here.

No other per-process-registration gap was found: neither project registers
anything else (a config-lookup worker, a config-backend page, a collection
backend/factory) anywhere but the one process that structurally needs it, and
this project's own equivalents (`JmapConfigLookup`, `JmapConfigServiceBackend`,
`JmapCollectionBackend`/`JmapCollectionFactory`,
`JmapBookBackend`/`Factory`, `JmapCalBackend`/`Factory`) already match
evolution-ews's registration set one-for-one on inspection of all five
`module*.rs`/`module-ews-*.c` files.

## Surface 2 — Account-setup backend vfuncs (`e-mail-config-ews-backend.c` vs `jmap-config/src/backend.rs`)

| EWS vfunc/feature | What it does | Our equivalent | Verdict | Reasoning |
|---|---|---|---|---|
| `backend_name = "ews"` | String the *Receiving Email* combo matches against Camel providers | `MAIL_BACKEND_NAME` ("jmap"), `backend.rs:196` | MATCH | Same mechanism. |
| `new_collection` | Bare `ESource` with `[Collection]`, only `backend_name` written | `new_collection`, `backend.rs:292` — writes a whole default account | DIVERGENCE — justified | `[Collection]` booleans default `false` unwritten; a bare-name-only source would read as "everything switched off," not the account the dialog shows. |
| `insert_widgets` bind target | Bound to the mail source's `CamelEwsSettings`, page-type-guarded | Bound to the **collection's** `[Authentication]`/`[Security]`, `backend.rs:471,503` | MATCH (pattern) / justified divergence in target | JMAP's server lives on the account, not a per-page mail source; documented at `backend.rs:407-419`. |
| OAB URL field, impersonation/delegate search, NTLM/GSSAPI/Office365 mechanism picker, Office365 tenant/client-ID/redirect/resource overrides | Exchange-specific UI | absent | no parity expected | No JMAP protocol analog; JMAP's OAuth2 registers a client dynamically per-server (RFC 7591), no fixed tenant/app-ID model to override. |
| Autodiscover button | Interactive Exchange Autodiscover, fills in Host URL | Not on this page — done by `JmapConfigLookup`, the assistant's separate "Look Up Account Details" step | DIVERGENCE — justified | Functional parity through a different, more integrated mechanism, not a gap. |
| `setup_defaults` | Unconditional overwrite of hosturl/email/user on every call | `setup_defaults`/`setup`, `backend.rs:936,1003` — writes host/user only when the address itself changed | DIVERGENCE — justified (deliberate improvement) | EWS's unconditional overwrite would clobber a user's manual server correction on an unchanged address; documented at `backend.rs:963-1002`. |
| `auto_configure` | Delegates to `e_mail_config_service_backend_auto_configure_for_kind` | `auto_configure`, `backend.rs:228-249` | MATCH | Same body, explicitly modeled on EWS's (fixed by `8936d12`). |
| `check_complete` | Per-entry inline hints (`e_util_set_entry_issue_hint`) | One freeform status label (`set_status_text`) | DIVERGENCE — justified | Same Next/Apply gating; presentation-granularity difference, not a missing check. |
| `commit_changes` | Copies email onto `CamelEwsSettings` — the only field EWS needs at commit time | `commit_changes`/`commit`, `backend.rs:1195,1228` — copies the whole `Connection` onto the receiving mail source; transport source is filled in later by the collection backend (`mail_child::follow_server`) | MATCH (was a documented open gap; **fixed doc, already-fixed code** — see below) | See finding below. |

**Finding, fixed this session (doc-only):** `backend.rs`'s own doc comment on
`commit_changes` stated "[the transport ending up with no server] is the next
increment, not something to fake here" — but the increment it was pointing at
had already landed, in a different crate, without this comment being updated:
`jmap_backend_collection::mail_child::follow_server` (added `8044513`, before
this very comment was written in `97b3995` — the two commits simply never
got reconciled) binds the transport (and mail account) sources' server fields
from the collection via `child_added`, exactly the mechanism this comment
said was still missing. Corrected the comment in place this session; no code
change, since the code was already correct. This is the same class of
finding as Surface 1's above — a documented "gap"/"reasoning" comment that
went stale after a later commit closed it, silently, in a file nobody thought
to cross-reference.

## Surface 3 — Config-lookup discovery (`e-ews-config-lookup.c` vs `jmap-config/src/config_lookup.rs`)

| EWS behavior/mechanism | Our equivalent | Verdict | Reasoning |
|---|---|---|---|
| Emits only `E_CONFIG_LOOKUP_RESULT_COLLECTION` | Same, `config_lookup.rs:402` | MATCH | |
| Priority `E_CONFIG_LOOKUP_RESULT_PRIORITY_IMAP - 100` = 900 | Hardcoded `900`, `config_lookup.rs:412` | MATCH | Same numeric outcome and rationale; our fix (`c85e916`) independently converged on EWS's exact value. Minor nit: EWS derives it from a real constant, we hardcode the number — not a functional gap. |
| `configure_source` copies host/port/user onto the auto-created Mail Account/Transport | No override — host/security instead come from the parent collection, read at connect time | DIVERGENCE — justified | Architectural: EWS's Camel settings are per-service; JMAP's are unified on the collection (`prepare_mail.rs`, `mail_child.rs`). |
| `servers` param: tries every entry in order | Tries only the first entry (`config_lookup.rs:164-168`) | DIVERGENCE — justified | Documented at lines 138-144: a JMAP deployment names exactly one issuer, unlike EWS/CalDAV where different servers may host different services. |
| Autodiscovery: authenticated Exchange Autodiscover (needs a password) | Unauthenticated RFC 8620 §2.2 SRV + RFC 8414/7591 discovery/registration | DIVERGENCE — justified | Correct per-protocol; JMAP discovery genuinely needs no credentials. |
| Seeds discovery from the existing collection's prior `hosturl` (`e_config_lookup_get_source`) | `run()` never reads any prior source, only `params` | DIVERGENCE — minor | Matters for re-running lookup on an edited account; low-impact since JMAP discovery is idempotent from email+servers alone. |
| Missing password → `E_CONFIG_LOOKUP_WORKER_ERROR_REQUIRES_PASSWORD`, assistant prompts and retries | **FIXED 2026-09-18** — `run()` now reports every discovery/registration failure once a host has actually been probed through its `error` out-parameter (`oauth2_setup::Error::to_gerror`, `G_IO_ERROR_CANCELLED`/`G_IO_ERROR_FAILED`); only the true non-match case (no email, empty domain) stays silent | MATCH (partial: reports a reason, does not retry) | EWS distinguishes "not a match" from "is a match but blocked," and reports the latter actionably. Our worker no longer collapses a plausible-but-blocked host into the same silence as a non-match; it does not implement `REQUIRES_PASSWORD`'s retry-with-credentials flow, since RFC 8414 discovery itself needs none. |
| Bad-certificate TLS error → extracts cert PEM/host into `*out_restart_params`, reports `E_CONFIG_LOOKUP_WORKER_ERROR_CERTIFICATE`, assistant offers trust-and-retry | No TLS-error path anywhere; `Error::Transport(String)` collapses TLS/DNS/I/O into one opaque string (now at least surfaced as a `G_IO_ERROR_FAILED` message, not swallowed); `_out_restart_params` is never written project-wide | DIVERGENCE — justified, but consequential | Explicitly documented security stance across three backends ("a certificate this code cannot see is one it must not invite anyone to accept"). Deliberate, not an oversight — but it means a self-hosted JMAP deployment with a self-signed/private-CA certificate cannot be onboarded via "Look Up Account Details" at all beyond a generic failure message, where EWS offers a trust-and-retry path. Explicitly out of scope (a product decision, not a bug fix). |

## Surface 4 — Camel provider registration (`camel-ews-provider.c` vs `jmap-mail/src/provider.rs`)

| EWS provider aspect | Our equivalent | Verdict | Reasoning |
|---|---|---|---|
| `CAMEL_PROVIDER_IS_REMOTE`/`IS_SOURCE`/`IS_STORAGE` | Same three, `provider.rs:76-79` | MATCH | |
| `CAMEL_PROVIDER_SUPPORTS_SSL` | We set it; EWS doesn't | DIVERGENCE — justified | EWS tunnels over HTTPS by convention with no user-facing toggle; JMAP's account UI exposes the choice explicitly (`provider.rs:71-75`). |
| `CAMEL_PROVIDER_IS_EXTERNAL` (means: appears in the folder tree but is not created by the mail component) | **FIXED 2026-09-18** — now set in `FLAGS` (`provider.rs`) | MATCH | `camel-enums.h`'s own doc comment for the flag ("appears in the folder tree but is not created by the mail component") is exactly the circumstance `provider.rs` already documented as true of every JMAP mail store: it is always spawned from a collection account's `ESource` extensions, never from the mail component's own New Mail Account wizard. No live-Evolution session was needed to confirm this reading; the header's own definition of the flag settles it. |
| `CamelProviderConfEntry` array | `extra_conf: null` | DIVERGENCE — justified | `provider.rs:152-154`: EDS 3.52 configures JMAP via `ESource` extensions, not legacy per-provider conf-entry widgets. Whether the underlying *features* (junk-on-fetch, folder-check-all, HTTP/1-only, etc.) exist elsewhere in jmap-mail is a separate feature-parity question, out of scope for this provider-registration-level surface. |
| `.url_flags` (EWS: `ALLOW_USER\|ALLOW_AUTH\|HIDDEN_HOST`) | `NEED_HOST\|ALLOW_PORT\|ALLOW_PATH\|ALLOW_USER\|ALLOW_AUTH\|ALLOW_PASSWORD` (`provider.rs:89-94`) | DIVERGENCE — justified | EWS hides the host (resolved via Autodiscover); JMAP requires an explicit host and allows a path (`/.well-known/jmap`), documented at `provider.rs:82-88`. |
| `authtypes` (NTLM/PLAIN/GSSAPI `CamelServiceAuthType` list) + `CAMEL_TYPE_SASL_XOAUTH2_OFFICE365` GType | `authtypes: null`, no `CamelSasl` subclass | DIVERGENCE — justified | JMAP authenticates as Basic or Bearer over plain HTTPS with no SASL handshake (`provider.rs:167-171`); OAuth2 is wired via `CamelNetworkSettings:auth-mechanism` + `camel_session_get_oauth2_access_token_sync` instead (`jmap-mail/src/oauth2.rs`), a complete substitute for what the SASL GType does for EWS. |
| protocol/name/description/domain, `translation_domain`, `object_types[STORE/TRANSPORT]` | Same shape, `provider.rs:41,58,65,138-149,195` | MATCH | |

**Verdict:** no unjustified provider-registration gap found; every divergence
traces to a real JMAP-vs-EWS protocol difference and is already documented in
`provider.rs`'s own comments.

## Surface 5 — Collection backend vfuncs (`e-ews-backend.c` vs `jmap-backend-collection/src/backend.rs`)

| EWS vfunc/behavior | What it does | Our equivalent | Verdict | Reasoning |
|---|---|---|---|---|
| `populate` | Claims cached resources, adds GAL/M365 helper sources, connects `"changed"` to force repopulate on edits, requests credentials or schedules authenticate | `backend.rs:201-253`, delegating to `crate::populate::populate` | **FIXED 2026-09-18** | Checked against EDS 3.52.3's own `e-collection-backend.c` (gitlab.gnome.org, tag `3.52.3`, since this VM only has headers): `collection_backend_notify_collection_cb` reschedules populate only for the three `ESourceCollection` properties `calendar-enabled`/`contacts-enabled`/`mail-enabled`; `collection_backend_online_changed_cb` reschedules on the online transition but throttled to once per 24h; construction and a manual `e_source_registry_refresh_backend()` call are the only unconditional triggers. There is no generic reschedule on the account's own `"changed"` signal. `e-ews-backend.c` (`master`) exists specifically to cover the gap: `ews_backend_populate` connects `source, "changed"` to `ews_backend_source_changed_cb`, which re-runs populate (subject to `need_update_folders`) on any account edit. This crate connects no such signal, so `populate.rs`'s module comment ("EDS itself reschedules... whenever the account changes") overstates what EDS actually does; a plain field edit (fixing a mistyped host, port, or similar) does not get a fresh credentials-required/authenticate cycle until the account is next disabled/re-enabled, goes online, or the user asks for a manual refresh. Fixed by `crate::source_changed` and `backend.rs`'s `connect_account_changed`/`on_account_changed`: the first populate of an account connects its `"changed"` to a handler that re-runs populate, and `AccountWatch` carries EWS's `need_update_folders` semantics (a populate marks the account unproven, a successful fan-out marks it proven, an edit only repopulates while unproven), so the edit that fixes a broken account gets a retry and a later rename costs nothing. The connection is made with `g_signal_connect_object` against the backend rather than EWS's `g_signal_connect` plus a stored `source_changed_id` disconnected in `dispose`: `g_object_watch_closure` refs the backend across the marshal and drops the closure from `g_object_real_dispose`, which is the same lifetime EWS hand-writes, minus the failure mode where the disconnect looks the source up again and finds a different one. Needs confirmation in a real Evolution session; there is no live registry here to drive a populate against. |
| `dup_resource_id` | Folder id off `ESourceEwsFolder` | `backend.rs:172-186` | MATCH | |
| `child_added` | Binds `[Authentication]` fields via live `GBinding`s, chains up **last** | `backend.rs:274-324`, chains up **first** | DIVERGENCE — justified | Order reversed deliberately (`backend.rs:262-268`): `offer_deletion` needs the parent's binding to already exist. |
| `child_removed` | Removes folder from EWS's own private id→source cache (delta-sync bookkeeping) | absent (`tests/backend.rs:369-371` pins the inherited/NULL slot) | DIVERGENCE — justified, confirmed 2026-09-18 | Read `e-ews-backend.c`'s `ews_backend_child_removed` (gitlab.gnome.org/GNOME/evolution-ews, `master`): it evicts an entry from `EEwsBackend`'s own folder-id-keyed hash table, used for its delta-sync bookkeeping. `jmap-collection-sync::Fanout` (`resources.rs`) carries no equivalent: `Fanout::discover` rebuilds `address_books`/`calendars` from scratch off a fresh JMAP session fetch every fan-out pass rather than mutating a persisted map, so there is no per-child cache entry a removal could leave stale or leaked. Verified against `Fanout`'s own fields, not just inferred from the fan-out call pattern — no work needed here. |
| `create_resource_sync`/`delete_resource_sync` | Server create/delete with foreign/public-folder special-casing, no chain-up | `backend.rs:422-549`, `571-660`, same non-chaining shape | MATCH | |
| `authenticate_sync` (grandparent `EBackendClass` slot) | Resolves credentials, on success calls `e_collection_backend_authenticate_children()` to push them into already-running address-book/calendar child backends immediately, then syncs | **FIXED 2026-08-24 (session N+58)** — `authenticate_with` (`authenticate.rs`) takes a `push_credentials` closure, called once a fan-out succeeds; `backend.rs`'s `authenticate_sync` wires it to `e_collection_backend_authenticate_children(collection.0, credentials)` | MATCH | Was a **GAP**; closed as item 11's own follow-up (1). |
| `EBackendClass::get_destination_address` | Parses the account's host into host/port, feeding EDS's own host-specific network-reachability monitor (rather than only generic network-up/down) | **FIXED 2026-08-24 (session N+57)** — `backend.rs`'s new `get_destination_address`, reading `jmap_backend_core::source::destination_address` (mirrors `ews_backend_get_destination_address`'s own `[Authentication] Host`/`Port` fallback branch) | MATCH | Was a **GAP**; closed as item 11's own follow-up (2). |
| `constructed` (sets `remote-creatable`, forces NTLM fallback, `allow-sources-rename=TRUE`, etc.) | absent | **`allow-sources-rename` FIXED 2026-09-18**; rest DIVERGENCE — justified | `backend.rs`'s `offer_creation` comment explicitly discusses and rejects a `constructed` override for `remote-creatable` specifically (an already-considered, reasoned decision). `allow-sources-rename` was a genuine minor omission, now closed the same way: `Populating::allow_rename`, called unconditionally from `populate` (`e-source-collection.c`, `3.52.3`, confirms the property is `G_PARAM_CONSTRUCT`, defaults `FALSE`, "meant mainly for GUI", and evolution-ews's own `constructed` sets it unconditionally to `TRUE`, so there is no per-account condition to compute here unlike `remote-creatable`). NTLM fallback stays out of scope: JMAP has no SASL/NTLM negotiation to force. |
| Module registration (`module-ews-backend.c`) | Backend, factory, OAuth2 service, plus a custom `ESourceEwsFolder` extension type | `module.rs`: backend, factory, OAuth2 service — no custom resource-id extension type | MATCH on ordering/rationale; unexplained asymmetry, not confirmed as a gap | See Surface 1. This crate's resource identity presumably rides on a built-in EDS extension rather than a bespoke one; not confirmed in this pass. |

**Two real gaps found, neither EWS-specific; both now fixed:**

1. **No `e_collection_backend_authenticate_children()`-equivalent push of
   freshly-resolved credentials to already-running child backends.** Before
   this fix, each child backend (book/cal/mail) independently fetched its own
   credentials via `connect_with`'s three-branch resolution when *it* needed
   them, rather than being handed what the collection backend just resolved.
   **Fixed 2026-08-24 (session N+58)** — see the table row above.
2. **`EBackendClass::get_destination_address` is not implemented**, leaving
   EDS's host-reachability monitor unable to watch this account's actual
   JMAP host specifically, only generic network-up/down. Low severity (the
   backend still works when the network is up and still fails cleanly when
   it's fully down; the gap is narrower "host X is down but the network
   generally isn't" detection), concretely scoped, and a reasonable
   next-increment candidate. **Fixed 2026-08-24 (session N+57)** — see the
   table row above.

Both were filed as follow-up items rather than fixed in the same session that
found them, per this project's own "each its own increment" discipline and
that session's time budget — each was picked up and closed in its own later
increment instead.

## Summary

Four of five named surfaces show close-to-exact parity, with every
divergence traceable to a real, already-documented JMAP-vs-EWS protocol
difference (unified-account-vs-per-service settings, unauthenticated SRV/RFC
8414/7591 discovery vs. authenticated Exchange Autodiscover, Bearer/HTTP
auth vs. wire-level SASL, dynamically-registered OAuth2 clients vs. a fixed
Office 365 app registration). The module-registration surface is now exact
four-for-four parity after item 12's fix, and this audit additionally found
and corrected two places where this project's own comments *mis-cited*
evolution-ews's actual behavior (claiming single-module OAuth2 registration
when EWS in fact registers in all four of the same processes this project
does) — the same root-cause shape as item 12's bug, caught here by reading
evolution-ews's real source instead of trusting a two-sessions-old paraphrase
of it. One further stale-doc bug was found and fixed: `jmap-config/src/
backend.rs`'s `commit_changes` doc described the transport-source-has-no-
server problem as still open, when `jmap_backend_collection::mail_child::
follow_server` had already closed it in an earlier, uncoordinated commit.

The collection-backend surface had two genuine, non-EWS-specific gaps, both
now fixed: no credential push to already-running child backends on a fresh
collection authentication (**fixed 2026-08-24, session N+58**), and no
`get_destination_address` override for host-specific reachability
monitoring (**fixed 2026-08-24, session N+57**). Both were filed as
follow-up items rather than fixed in the audit itself. The config-lookup
surface's failure-mode differentiation gap (a JMAP-shaped host that fails
discovery for a real reason vs. a plain non-match) is **fixed 2026-09-18**:
`JmapConfigLookup::run` now reports a `GError` once a host has actually been
probed and discovery/registration then fails, rather than staying silent.

This surface's two remaining open questions were both resolved 2026-09-18
by reading EDS 3.52.3's and evolution-ews's actual source rather than
trusting this project's own paraphrase of either: `child_removed`'s absent
cache-eviction is confirmed harmless (`Fanout` keeps no persisted per-child
state to evict), and the `populate` row's "does EDS reschedule on a plain
field edit" question is confirmed **no**. That gap is **fixed 2026-09-18**:
`crate::source_changed` connects the account's own `"changed"` and re-runs
populate while the account's settings are still unproven, so correcting a
mistyped host mid-session gets a fresh credentials-required cycle instead of
waiting for the account to be disabled and re-enabled. The wiring itself
still wants a real Evolution session to confirm, for the reason the row
gives.
