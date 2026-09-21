<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

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
| Module registration (`module-ews-backend.c`) | Backend, factory, OAuth2 service, plus a custom `ESourceEwsFolder` extension type | `module.rs`: backend, factory, OAuth2 service — no custom resource-id extension type | DIVERGENCE — justified, confirmed 2026-09-18 | See Surface 1 for the shared part. Checked `e-source-ews-folder.h`/`.c` (gitlab.gnome.org/GNOME/evolution-ews, tag `3.52.3`) for what the bespoke type actually carries: `change_key`, `id`, `name`, `foreign`/`foreign_subfolders`/`foreign_mail` (opening a delegate's mailbox), `public` (public-folder flag), `use_primary_address`, `fetch_gal_photos`, `freebusy_weeks_before`/`freebusy_weeks_after`. Every one of those beyond plain identity is an Exchange-specific concept this project already documents as out of scope elsewhere in this file (foreign/public-folder handling, row 143; GAL, no protocol analog) — JMAP has no delegate-mailbox, public-folder, or GAL-photo notion for a bespoke extension to carry. The remaining role, naming a child resource, is exactly what `[Resource] Identity` already does (`resource_id.rs`'s own module doc walks through why the built-in `ESourceResource` extension is enough: a single string identity plus the kind read off `[Address Book]`/`[Calendar]`, no per-kind sync bookkeeping this backend's collection layer needs to persist on the `ESource` itself). No custom extension type has anything left to carry. |

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

## Surface 6 - Address-book backend vfuncs (`e-book-backend-ews.c` vs `jmap-backend-book/src/backend.rs`)

Both backends are direct subclasses of EDS's `EBookMetaBackend` (`E_TYPE_BOOK_META_BACKEND`), which provides caching (via SQLite `EBookCache`), offline operation, and view dispatching over a set of core synchronization and contact-manipulation vfuncs. Upstream source references are to `src/EWS/addressbook/e-book-backend-ews.c` and `src/EWS/addressbook/e-book-backend-ews-factory.c` on `master`. References into this repository are relative to `rust/crates/`.

| EWS vfunc/behavior | What it does | Our equivalent | Verdict | Reasoning |
|---|---|---|---|---|
| `connect_sync` | Validates credentials, connects push/streaming notifications, sets `writable = !is_gal`, runs cache data version migration (`e-book-backend-ews.c:3792-3903`) | `connect_sync`, `jmap-backend-book/src/backend.rs:280-350` (delegating to `connect::connect`, `start_push`, and `e_book_backend_set_writable`) | MATCH | Same connection lifecycle, credential handling, push initialization, and writable assignment. JMAP has no read-only GAL, so writable is always set TRUE. TLS certificate out-parameters are left untouched on both backends per this project's documented security stance. |
| `disconnect_sync` | Unsubscribes notifications, unrefs connection, cancels pending view operations (`e-book-backend-ews.c:3905-3920`) | `disconnect_sync`, `jmap-backend-book/src/backend.rs:352-375` (stops push thread, drops connection) | MATCH | Same clean disconnect pattern. |
| `list_existing_sync` | Absent (NULL in `EBookMetaBackendClass`, `e-book-backend-ews.c:4740-4748`) | `list_existing_sync`, `jmap-backend-book/src/backend.rs:377-394` (delegating to `ops::list_existing`) | DIVERGENCE - justified | Protocol architecture: Exchange's `SyncFolderItems` operation accepts a null sync state to enumerate all existing items, so EWS needs only `get_changes_sync`. JMAP separates `ContactCard/changes` (delta sync) from `ContactCard/query` + `ContactCard/get` (full listing). `jmap-backend-book` implements `list_existing_sync` and chains up to parent `EBookMetaBackendClass::get_changes_sync` when `cannotCalculateChanges` requires a full re-sync. |
| `get_changes_sync` | For GAL: downloads OAL details/diffs; for address books: calls `e_ews_connection_sync_folder_items_sync`, verifies changes against cache, fetches created/modified items (`e-book-backend-ews.c:3923-4148`) | `get_changes_sync`, `jmap-backend-book/src/backend.rs:397-455` (delegating to `ops::get_changes`, chains up to parent on `ListInstead`) | MATCH (pattern) / justified divergence in sync mechanics | Both implement incremental change synchronization feeding `EBookMetaBackendInfo` lists to EDS. EWS handles Exchange OAB/GAL and `SyncFolderItems`; JMAP uses `ContactCard/changes` and chains up to parent listing when changes cannot be calculated. Both wrap calls in token refresh retry logic. |
| `load_contact_sync` | Fetches item via `e_ews_connection_get_items_sync`, converts EWS XML to `EContact`, caches original vCard (`e-book-backend-ews.c:4151-4202`) | `load_contact_sync`, `jmap-backend-book/src/backend.rs:457-476` (delegating to `ops::load_contact`) | MATCH | Same single-contact retrieval into `EContact`. |
| `save_contact_sync` | Creates or updates contact via `CreateItem`/`UpdateItem`, handles distribution lists (`E_CONTACT_IS_LIST`), uploads photo attachment for Exchange 2010 SP2+ (`e-book-backend-ews.c:4205-4349`) | `save_contact_sync`, `jmap-backend-book/src/backend.rs:479-512` (delegating to `ops::save_contact`) | MATCH (pattern) / justified divergence | Both translate `EContact` modifications into protocol create/update calls. EWS handles Exchange distribution lists and custom photo attachment uploads. JMAP maps `EContact` to JSContact Card representations via `ContactCard/set` with last-writer-wins conflict resolution. |
| `remove_contact_sync` | Deletes contact via `e_ews_connection_delete_items_sync` with `EWS_HARD_DELETE` (`e-book-backend-ews.c:4351-4383`) | `remove_contact_sync`, `jmap-backend-book/src/backend.rs:514-535` (delegating to `ops::remove_contact`) | MATCH | Both issue remote deletion and report outcome cleanly. |
| `search_sync` and `search_uids_sync` | Overridden to query remote GAL via `e_ews_connection_resolve_names_sync`; for non-GAL, immediately chains up to parent class (`e-book-backend-ews.c:3419-3432, 4385-4455`) | Absent (inherited from `EBookMetaBackendClass`) | DIVERGENCE - justified | EWS overrides these slots solely to support live remote search against the Exchange GAL (`e-book-backend-ews.c:3431`). For standard address books, EWS chains up directly to `EBookMetaBackendClass::search_sync`/`search_uids_sync`, which queries the local SQLite `EBookCache`. JMAP has no GAL; all contacts are synchronized locally, so inheriting the parent class yields exact parity for non-GAL address books. |
| `impl_get_backend_property` | Returns `CLIENT_BACKEND_PROPERTY_CAPABILITIES` (`net`, `contact-lists`, `do-initial-query`), `REQUIRED_FIELDS` (`file-as`), `SUPPORTED_FIELDS` (`e-book-backend-ews.c:4457-4530`) | Absent (inherited from `EBookBackendClass`) | DIVERGENCE - justified / minor gap | `EBookMetaBackend` provides default capabilities (`net`, `contact-lists` if enabled). EWS explicitly enumerates `SUPPORTED_FIELDS` to restrict the Evolution contact editor to fields supported by its XML schema. JSContact supports standard contact fields natively, but explicit `SUPPORTED_FIELDS` advertising could be added if field gating is desired. |
| `impl_start_view` and `impl_stop_view` | Overridden to run asynchronous background GAL search when view has `MANUAL_QUERY` flag; otherwise chains up to parent class (`e-book-backend-ews.c:4566-4624`) | Absent (inherited from `EBookBackendClass`) | DIVERGENCE - justified | Exists exclusively for live remote GAL search in EWS (`e-book-backend-ews.c:4574`). Standard address-book views are managed entirely by `EBookMetaBackendClass` against `EBookCache`. |
| `EBackendClass::get_destination_address` | Extracts host and port from `CamelEwsSettings` hosturl for EDS host-reachability monitoring (`e-book-backend-ews.c:4627-4667`) | Absent in `jmap-backend-book/src/backend.rs` | GAP (minor) | In `evolution-addressbook-factory`, `EBackendClass`'s default implementation reads the backend's "connectable" property, which is never set, so EDS host-reachability monitoring falls back to generic network-up/down rather than watching the specific JMAP host. Identical to the gap found on Surface 5 (Collection backend vfuncs); `jmap_backend_core::source::destination_address` already exists and can be wired into `JmapBookBackendClass` as a follow-up item. |
| `constructed`, `dispose`, `finalize` | `constructed` creates attachments directory for contact photos; `dispose` unsets connection; `finalize` frees folder id and allocations (`e-book-backend-ews.c:4670-4716`) | `instance_init` initializes session/push slots; `finalize` stops push thread and clears session (`jmap-backend-book/src/backend.rs:238-260`) | MATCH (pattern) / justified divergence | JMAP needs no disk attachments directory for contact photos. Push thread and session teardown occur safely in `finalize`. |
| Direct Read Access (DRA) configuration | Sets `backend_module_directory`, `backend_module_filename`, and `backend_factory_type_name` (`e-book-backend-ews.c:4737-4739`) | Absent (left NULL in `JmapBookBackendClass`) | DIVERGENCE - justified | DRA allows client processes to open local cache databases directly without IPC. Leaving these NULL is standard in EDS when in-process client loading is not desired, routing all operations cleanly through `evolution-addressbook-factory`. |
| Subprocess sharing (`share_subprocess`) | Sets `share_subprocess = TRUE` in factory class init (`e-book-backend-ews-factory.c:42`) | Left `FALSE` (default) in `jmap-backend-book/src/factory.rs:112-117` | DIVERGENCE - justified (deliberate improvement) | Setting `share_subprocess = TRUE` puts all accounts into a single factory subprocess. Leaving it `FALSE` isolates each JMAP account source in its own subprocess, providing credential separation and minimizing fault blast radius. |

**One real gap found:**

1. **`EBackendClass::get_destination_address` is not implemented on `JmapBookBackendClass`**, leaving EDS's host-reachability monitor unable to watch this account's actual JMAP host specifically inside `evolution-addressbook-factory`, falling back to generic network-up/down. This is the exact counterpart to the gap found on Surface 5 for `jmap-backend-collection`. The logic already exists in `jmap_backend_core::source::destination_address` (added in session N+57), so wiring it to `JmapBookBackendClass`'s `parent_class` is a scoped follow-up increment.

## Surface 7 - Calendar backend vfuncs (`e-cal-backend-ews.c` vs `jmap-backend-cal/src/backend.rs`)

Both backends are subclasses of EDS's `ECalMetaBackend` (`E_TYPE_CAL_META_BACKEND`), which provides caching (via SQLite `ECalCache`), offline operation, and view dispatching over core synchronization and calendar component manipulation vfuncs. Upstream source references are to `src/EWS/calendar/e-cal-backend-ews.c` and `src/EWS/calendar/e-cal-backend-ews-factory.c` on `master`. References into this repository are relative to `rust/crates/`.

| EWS vfunc/behavior | What it does | Our equivalent | Verdict | Reasoning |
|---|---|---|---|---|
| `connect_sync` | Validates credentials, connects push/streaming notifications (`server-notification`), sets `writable = !is_freebusy_calendar` (`e-cal-backend-ews.c:1779-1870`) | `connect_sync`, `jmap-backend-cal/src/backend.rs:329-404` (delegating to `connect::connect`, `start_push`, and `e_cal_backend_set_writable`) | MATCH | Same connection lifecycle, credential handling, push subscription initialization, and writable assignment (`e_cal_backend_set_writable`). JMAP sets writable TRUE unconditionally since JMAP has no synthetic read-only freebusy folders. TLS certificate out-parameters are left untouched on both backends per documented security stance. |
| `disconnect_sync` | Unsubscribes notifications, unrefs connection, cancels pending operations (`e-cal-backend-ews.c:1873-1886`) | `disconnect_sync`, `jmap-backend-cal/src/backend.rs:406-430` (stops push thread, drops connection) | MATCH | Clean unsubscription and socket/connection release on both implementations. |
| `list_existing_sync` | Absent (NULL in `ECalMetaBackendClass`, `e-cal-backend-ews.c:4777-4785`) | `list_existing_sync`, `jmap-backend-cal/src/backend.rs:432-449` (delegating to `ops::list_existing`) | DIVERGENCE - justified | Protocol architecture: Exchange's `SyncFolderItems` operation accepts a null sync state to enumerate all existing items, so EWS needs only `get_changes_sync`. JMAP separates `CalendarEvent/changes` (delta sync) from `CalendarEvent/query` + `CalendarEvent/get` (full listing). `jmap-backend-cal` implements `list_existing_sync` and chains up to parent `ECalMetaBackendClass::get_changes_sync` when `cannotCalculateChanges` requires a full re-sync. |
| `get_changes_sync` | For freebusy calendars: queries free/busy ranges via `e_ews_connection_get_free_busy_sync` and creates synthetic VEVENTs; for standard calendars: calls `e_ews_connection_sync_folder_items_sync`, fetches created/modified items (`e-cal-backend-ews.c:1889-2274`) | `get_changes_sync`, `jmap-backend-cal/src/backend.rs:452-511` (delegating to `ops::get_changes`, chains up to parent on `ListInstead`) | MATCH (pattern) / justified divergence in sync mechanics | Both implement incremental change synchronization feeding `ECalMetaBackendInfo` lists to EDS. EWS handles synthetic freebusy calendars and `SyncFolderItems`; JMAP uses `CalendarEvent/changes` and chains up to parent listing when deltas cannot be computed. Both wrap calls in token refresh retry logic. |
| `load_component_sync` | Fetches item via `e_ews_connection_get_items_sync`, converts EWS XML items to `ICalComponent` (`e-cal-backend-ews.c:2277-2348`) | `load_component_sync`, `jmap-backend-cal/src/backend.rs:513-532` (delegating to `ops::load_component`) | MATCH | Same single-component retrieval into `ICalComponent` (mapping via `jmap-ical` in JMAP). |
| `save_component_sync` | Creates or updates item via `CreateItem`/`UpdateItem`, diffs recurrence exceptions, handles online meeting M365 properties (`e-cal-backend-ews.c:3144-3486`) | `save_component_sync`, `jmap-backend-cal/src/backend.rs:535-577` (delegating to `ops::save_component`) | MATCH (pattern) / justified divergence | Both translate `ECalComponent` modifications into protocol create/update calls. Both resolve timezones using the backend timezone cache. JMAP maps `ECalComponent` instances (master + overrides) to JSCalendar Event representations via `CalendarEvent/set` with last-writer-wins conflict resolution. |
| `remove_component_sync` | Deletes component via `e_ews_connection_delete_item_sync` with `EWS_HARD_DELETE` and sends cancellations if configured (`e-cal-backend-ews.c:3489-3532`) | `remove_component_sync`, `jmap-backend-cal/src/backend.rs:579-600` (delegating to `ops::remove_component`) | MATCH | Both issue remote component deletion (`CalendarEvent/set` destroy in JMAP) and report outcome cleanly. |
| `source_changed` | Checks if freebusy calendar window (`freebusy_calendar_weeks_before`/`after`) changed on `ESourceEwsFolder` extension and schedules refresh (`e-cal-backend-ews.c:3535-3562`) | `source_changed`, `jmap-backend-cal/src/backend.rs:602-657` (detects colour changes via `marshal::selectable_color` and calls `ops::on_source_changed`) | MATCH (pattern) / justified divergence | Both handle EDS `source_changed` notifications on the dedicated worker thread to push local `ESource` configuration changes. EWS monitors freebusy time range adjustments; JMAP synchronizes user-selected calendar color changes back to the remote server via `Calendar/set`. |
| `get_free_busy_sync` (`ECalBackendSyncClass`) | Calls `e_ews_connection_get_free_busy_sync` and formats VFREEBUSY components into string list (`e-cal-backend-ews.c:4478-4541`) | `get_free_busy_sync`, `jmap-backend-cal/src/backend.rs:673-723` (delegating to `ops::get_free_busy`, chains up to parent cache when `NothingKnown`) | MATCH (pattern) / justified divergence | Both implement free/busy scheduling lookups over requested users and time ranges. EWS queries Exchange Web Services directly. JMAP queries `ops::get_free_busy` and falls back cleanly to chaining up to `ECalMetaBackendClass::parent_class.get_free_busy_sync` to compute busy intervals from the local cache when offline or unconfigured. |
| `get_timezone_sync` (`ECalBackendSyncClass`) | Resolves timezone via parent class, falling back to translating Windows zone names to libical/Olson equivalents via `windowsZones.xml` (`e-cal-backend-ews.c:4596-4625`) | Absent (inherited from `ECalBackendSyncClass`) | DIVERGENCE - justified | EWS overrides this slot solely to translate legacy Windows timezone identifiers to libical equivalents. JMAP/JSCalendar uses standard IANA timezone identifiers exclusively (RFC 8984), so inheriting the parent class timezone resolution is exact. |
| `receive_objects_sync` and `send_objects_sync` (`ECalBackendSyncClass`) | Handles inbound meeting invitations/updates/replies via `CreateItem` (AcceptItem, DeclineItem, TentativelyAcceptItem); sends meeting cancellations via `ecb_ews_send_cancellation_email_sync` (`e-cal-backend-ews.c:4167-4475`) | Absent (inherited from `ECalBackendSyncClass`, returning `E_CLIENT_ERROR_NOT_SUPPORTED`) | DIVERGENCE - justified / roadmap boundary | EWS implements server-side meeting response handling and cancellation email transmission. JMAP calendar backend explicitly documents that iTIP scheduling operations are deferred to a future milestone (`backend.rs:544`, `ops.rs:26`). |
| `discard_alarm_sync` (`ECalBackendSyncClass`) | Issues `UpdateItem` clearing `ReminderIsSet` on the remote item or occurrence (`e-cal-backend-ews.c:3565-3650`) | Absent (inherited from `ECalBackendSyncClass`) | DIVERGENCE - justified / minor gap | EWS propagates alarm dismissal to Exchange. JMAP/JSCalendar models alarms in the `alerts` map (RFC 8984 Section 4.5), where acknowledgment updates the `acknowledged` timestamp. In the current implementation, reminder dismissals are handled locally by Evolution's alarm daemon without mutating the remote event's `alerts` map. |
| `impl_get_backend_property` (`ECalBackendClass`) | Advertises static capabilities (`CLIENT_BACKEND_PROPERTY_CAPABILITIES`: `NO_EMAIL_ALARMS`, `NO_AUDIO_ALARMS`, `NO_PROCEDURE_ALARMS`, `ONE_ALARM_ONLY`, `NO_THISANDPRIOR`, `NO_THISANDFUTURE`, etc.) and `CAL_EMAIL_ADDRESS` (`e-cal-backend-ews.c:4544-4593`) | Absent (inherited from `ECalBackendClass`) | DIVERGENCE - justified / minor gap | `ECalMetaBackend` provides default capabilities and resolves `CAL_EMAIL_ADDRESS` from the `ESource`. EWS explicitly advertises static capabilities to constrain the Evolution calendar UI. Specifically, advertising `NO_THISANDFUTURE` and `NO_THISANDPRIOR` in JMAP would align Evolution UI options with JSCalendar's recurrence override model (Section 13.427) by disabling unsupported range modifications. |
| `EBackendClass::get_destination_address` | Extracts host and port from collection settings for host-specific reachability monitoring (`e-cal-backend-ews.c:4628-4668`) | Absent in `jmap-backend-cal/src/backend.rs` | GAP (minor) | In `evolution-calendar-factory`, EDS host-reachability monitoring falls back to generic network-up/down rather than watching the specific JMAP host. Identical to the gap found on Surface 5 (Collection backend) and Surface 6 (Address-book backend); `jmap_backend_core::source::destination_address` already exists and can be wired into `JmapCalBackendClass`. |
| `constructed`, `dispose`, `finalize` (`GObjectClass`) | `constructed` sets up attachment folder and cache revision signal; `dispose` disconnects; `finalize` frees folder ID and attachment paths (`e-cal-backend-ews.c:4681-4755`) | `instance_init` initializes session/color/push slots; `finalize` stops push thread and clears session/color (`jmap-backend-cal/src/backend.rs:281-310`) | MATCH (pattern) / justified divergence | JMAP needs no disk attachments directory for calendar items. Push thread and session teardown occur safely in `finalize`. |
| Subprocess sharing (`share_subprocess`) | Sets `share_subprocess = TRUE` in factory class init (`e-cal-backend-ews-factory.c:62,86,110`) | Left `FALSE` (default) in `jmap-backend-cal/src/factory.rs:135-141` | DIVERGENCE - justified (deliberate improvement) | Setting `share_subprocess = TRUE` puts all accounts into a single factory subprocess. Leaving it `FALSE` isolates each JMAP account source in its own subprocess, providing credential separation and minimizing fault blast radius. |
| Component kinds (`component_kind`) | Registers three factories: `VEVENT` (events), `VJOURNAL` (memos), and `VTODO` (tasks) (`e-cal-backend-ews-factory.c:65,89,113`) | Registers single factory for `VEVENT` (`jmap-backend-cal/src/factory.rs:41-53`) | DIVERGENCE - justified | JMAP calendars RFC models `CalendarEvent` (RFC 8984) for events. JMAP currently has no standardized task or memo mapping; registering non-functional factories would produce empty, broken calendar views. |

**One real gap found:**

1. **`EBackendClass::get_destination_address` is not implemented on `JmapCalBackendClass`**, leaving EDS's host-reachability monitor unable to watch this account's actual JMAP host specifically inside `evolution-calendar-factory`, falling back to generic network-up/down. This is the exact counterpart to the gap found on Surface 5 for `jmap-backend-collection` and Surface 6 for `jmap-backend-book`. The logic already exists in `jmap_backend_core::source::destination_address` (added in session N+57), so wiring it to `JmapCalBackendClass`'s `parent_class` is a scoped follow-up increment.

## Summary

Six of seven named surfaces show close-to-exact parity, with every
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

The module-registration row's own open question is closed too, 2026-09-18:
evolution-ews's bespoke `ESourceEwsFolder` extension type carries mostly
Exchange-specific fields (delegate-mailbox, public-folder, GAL-photo,
free-busy-range) this project has no protocol analog for, and the one
role it shares with this project, naming a child resource, is already
covered by the built-in `ESourceResource` extension this crate uses. No
gap; nothing left open in this document.

The address-book backend surface (Surface 6) shows comprehensive parity across
the standard `EBookMetaBackendClass` lifecycle and CRUD vfuncs (`connect_sync`,
`disconnect_sync`, `get_changes_sync`, `load_contact_sync`, `save_contact_sync`,
`remove_contact_sync`). The divergences in `search_sync`, `search_uids_sync`,
`impl_start_view`, and `impl_stop_view` trace entirely to EWS's Exchange Global
Address List (GAL) support: EWS overrides those slots exclusively to execute
live remote directory searches against the GAL, while falling back to the
parent class defaults for standard address books, matching JMAP's direct use
of the local cache. The `list_existing_sync` divergence is architectural:
Exchange's `SyncFolderItems` unifies initial and incremental sync, while JMAP
separates query/get from delta changes, chaining up to parent listing when a
full sync is needed. One genuine minor gap was identified:
`EBackendClass::get_destination_address` is not implemented on
`JmapBookBackendClass`, repeating the exact omission identified in Surface 5
where host-specific reachability monitoring falls back to generic network state
in `evolution-addressbook-factory`.

The calendar backend surface (Surface 7) shows comprehensive parity across
the standard `ECalMetaBackendClass` lifecycle and CRUD vfuncs (`connect_sync`,
`disconnect_sync`, `get_changes_sync`, `load_component_sync`, `save_component_sync`,
`remove_component_sync`), as well as `source_changed` (used in JMAP for
calendar color synchronization) and `get_free_busy_sync`. The divergences in
`get_timezone_sync`, `receive_objects_sync`, `send_objects_sync`, and
`discard_alarm_sync` trace to real protocol differences: EWS uses Windows
timezone translation tables and Exchange meeting response items, while JMAP
relies on standard IANA timezones and defers full iTIP scheduling to a future
milestone. One genuine minor gap was identified:
`EBackendClass::get_destination_address` is not implemented on
`JmapCalBackendClass`, repeating the exact omission identified in Surface 5 and
Surface 6 where host-specific reachability monitoring falls back to generic
network state in `evolution-calendar-factory`.
