<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# FFI soundness audit (Track A5, 2026-08-19)

Scope, as named by Track A5: for every `extern "C"`
vfunc trampoline and FFI call site across the workspace's EDS-integration
crates, check four things —

1. every vfunc trampoline is `catch_unwind`-wrapped (a Rust panic must never
   cross into C);
2. transfer-full vs. transfer-none ownership is honoured on every returned
   GObject/string/list (`g_free`/`g_object_unref` correctness);
3. nullability is checked at each FFI boundary a C API can hand back NULL;
4. `GCancellable` is honoured on the sync vfuncs that receive one.

This complements `docs/UNSAFE-AUDIT.md` (Track A6), which is about
*reduction/idiom* (fewer, more consolidated unsafe call sites) rather than
*soundness*. Several of A6's own findings — the `Owned<T>` RAII wrapper
(Pattern C) and the `checked_borrow`/`dispatched_borrow`/`extension_if_present`
helper families (Patterns B/D) — narrow this audit's job considerably: sites
that already go through those helpers are null-checked and (for `Owned<T>`)
transfer-correct by construction, so this audit did not re-derive that,
only confirmed nothing bypasses them with a raw unchecked cast.

**Method.** Read every `unsafe fn`/`extern "C" fn` and FFI call site in
`jmap-backend-core`, `jmap-backend-book`, `jmap-backend-cal`,
`jmap-backend-collection` + `jmap-collection-sync`, `jmap-mail` +
`jmap-mail-sync`, and `jmap-config` + `jmap-config-module` — every crate that
carries EDS/GObject/Camel FFI surface (`eds-sys`/`evo-sys` are out of scope
by their own doc comment: the bindgen-generated bindings are the one place
*meant* to be audited against the C ABI directly, and `eds-sys/src/compat.rs`
was already covered by A6's per-crate summary). Cross-checked transfer
annotations against the installed `.gir` files and, where no `.gir` entry
existed (a handful of libecal/EDS internals), against the vendored EDS
3.52.4 C source rather than trusting in-repo comments alone.

## Question 1 — catch_unwind coverage

**Already answered, by Track A6.** `docs/UNSAFE-AUDIT.md`'s Pattern F
finding stands: every `extern "C"` vfunc/trampoline in every production
crate is routed through `jmap_backend_core::trampoline::{guard, guard_bool,
guard_ptr, guard_value}`. This audit's own crate-by-crate read reconfirms
it, with one narrow correction (below) rather than a counter-example.

## Question 2 — transfer correctness

**Clean, with one enforcement gap noted rather than an active bug.**
`jmap-backend-book`, `jmap-backend-cal`, `jmap-backend-collection`,
`jmap-collection-sync`, and `jmap-mail`/`jmap-mail-sync` all came back with
**zero findings** — every transfer-full pointer/list this audit traced
(`e_backend_get_source`, `e_collection_backend_ref_server`,
`e_collection_backend_claim_all_resources`,
`e_source_registry_server_ref_credentials_provider`,
`e_source_credentials_provider_lookup_sync`, `camel_folder_summary_get`,
`camel_data_cache_get`/`_add`, every libical-glib getter
`jmap-backend-cal/marshal.rs` wraps in `Owned<T>`) is released on every exit
path, matching its `.gir`/source-documented annotation, and every
transfer-none pointer is left alone. `jmap-config`/`jmap-config-module` came
back clean too (GTK widget refs sunk on attach, `ESource` extensions treated
as borrowed throughout, `new_collection`'s freshly-created `ESource` handed
off transfer-full correctly).

The one gap found, in `jmap-backend-core`:

- ~~**`error.rs::set_raw_gerror`'s "`*dest` must already be NULL" precondition
  is enforced only by `debug_assert!`, which compiles out in release
  builds.**~~ **Fixed 2026-08-20.** A third branch now handles an already-set
  `*dest`: log a critical via `trampoline::log_critical` and free the
  incoming `error`, keeping the first one — matching GLib's own
  `g_set_error()` family, which refuses the same way at runtime, not only in
  debug builds, and this crate's own existing idiom for "cannot happen but
  must not be UB if it somehow does."

## Question 3 — nullability

**Clean.** No unchecked `.cast::<T>().as_ref()` or unwrap on FFI-supplied
data was found anywhere in scope beyond what Track A6's helper
consolidation (`checked_borrow*`, `dispatched_borrow`, `extension_if_present`)
already covers correctly. Every `CStr::from_ptr`/array-walk this audit
traced by hand (outside those helpers) is preceded by an explicit
null check. One recurring, correct pattern worth naming since it looks like
an unchecked read at a glance: several sites (`jmap-backend-core/source.rs`,
`jmap-backend-collection/child_source.rs::extension`,
`mail_child.rs::follow_server`'s child-side reads, `jmap-config`'s extension
getters) call `e_source_get_extension` without a null check *because* the
corresponding extension's `_get_type()` is referenced immediately above,
which is EDS's own documented precondition for that getter never returning
NULL — a real guarantee, applied consistently, not a gap.

`jmap-config/src/backend.rs::insert_entries`'s ~25-unsafe-block GTK page
function was specifically re-examined given its size: every widget is
attached to a container immediately after creation (sinking the floating
ref), every `e_binding_bind_property(_full)` return is correctly discarded,
and the one internal inconsistency found — the `[Authentication]`/
`[Security]` extension pointers get an explicit NULL guard before the
`g_signal_connect_object` calls but not before the earlier
`e_binding_bind_property` calls a few lines above — is not a live bug for
the reason above (the extension types are guaranteed-registered), just
asymmetric defensiveness. **Tag: INVESTIGATE, cosmetic**, not fixed here —
harmonizing the two blocks is a one-file readability pass, not a soundness
fix, and not worth its own increment.

## Question 4 — GCancellable

**Clean.** Every sync vfunc across all six crate-groups that declares a
`*mut GCancellable` was checked; each one either routes it through
`jmap_backend_core::cancel::observe`/`CancelBridge` before any
network-touching work, or correctly passes it straight through to a nested
native-GIO sync call that honours its own cancellable
(`oauth2::access_token` → `e_source_get_oauth2_access_token_sync`,
`jmap-mail/service.rs::connect_sync`/`disconnect_sync` →
`camel_session_authenticate_sync`/the parent's `disconnect_sync`,
`folder.rs::search_by_expression`/`search_by_uids` → `camel_folder_search_search`,
which does local in-memory work with nothing async to bridge).
`jmap-backend-collection`'s three cancellable-taking vfuncs
(`create_resource_sync`, `delete_resource_sync`, `authenticate_sync`) all
install the bridge for exactly the span of the network-touching call, not
before. `jmap-config/config_lookup.rs::run` is the one function in that
crate taking a cancellable and it is correctly bridged.

## Findings summary

| # | Where | Category | Confidence | Status |
|---|---|---|---|---|
| 1 | `jmap-backend-core/src/error.rs::set_raw_gerror` | transfer/precondition enforcement | LOW (no live bug; hardening) | **fixed 2026-08-20** |
| 2 | `jmap-config/src/oauth2_service.rs::get_name`/`get_display_name`, `config_lookup.rs::get_display_name` | catch_unwind coverage | LOW (theoretical; both bodies are currently infallible) | **fixed this session** |
| 3 | `jmap-config/src/oauth2.rs::borrowed` | concurrency / pointer lifetime | **CONFIRMED — use-after-free demonstrated** | **fixed 2026-08-20** |
| 4 | `jmap-config/src/backend.rs::insert_entries` | nullability consistency | LOW (cosmetic) | logged, not fixed |

### Finding 2 — fixed: three vtable functions were not routed through `guard`

Of 24 `unsafe extern "C"` functions across `jmap-config`, three were calling
their (currently infallible) bodies directly rather than through
`trampoline::guard`, a mechanical exception to the crate's own
otherwise-universal convention: `oauth2_service.rs::get_name`,
`oauth2_service.rs::get_display_name`, and
`config_lookup.rs::get_display_name`. Neither body can plausibly panic
today (`get_name` returns a `'static` pointer; the two `get_display_name`s
call `i18n::translate_static`, itself a thin `dgettext` wrapper), so this
was not a live bug — but nothing stops a future edit to either body from
adding fallible logic and unwinding straight into GObject/EDS with no
guard to catch it, which is exactly the invariant Pattern F's audit finding
says this codebase otherwise holds everywhere. Wrapped all three in `guard`,
matching every sibling vtable function in the same files
(`get_client_id`/`get_client_secret`/`get_authentication_uri`/
`get_refresh_uri`/`get_redirect_uri` in `oauth2_service.rs`; `constructed`/
`run` in `config_lookup.rs`). No behaviour change: `guard`'s fallback on the
(unreachable-today) panic path is the same `ptr::null()` GIO already treats
as "no name". `jmap-config`'s existing test suite passed unmodified.

### Finding 3 — logged, not fixed: `oauth2.rs::borrowed`'s pointer lifetime under concurrent `apply()`

`docs/UNSAFE-AUDIT.md` already flagged this as INVESTIGATE ("no lock needed
against concurrent mutation... plausible... not pinned by a test"). This
audit's read agrees it is real and sharpens the mechanism: `borrowed()`
locks `Fields`' mutex, reads the requested field, *releases the lock*, and
returns a raw `*const c_char` pointing into the `CString` that lock was
protecting. `EOAuth2Service` vtable methods (`get_client_id` and friends)
can be invoked by EDS's OAuth2 machinery from a worker thread while
Evolution's main thread concurrently runs `oauth2::apply()` (e.g. rerunning
discovery after the user edits settings), which replaces the whole `Fields`
struct — dropping the `CString` a C caller may still be holding a pointer
into.

**Not fixed here, deliberately.** This is exactly the kind of subtle
cross-thread pointer-lifetime reasoning the night-shift escalation criteria
name explicitly — a plausible-but-wrong fix (e.g. shrinking the lock's
scope without changing what's returned) would look correct and compile
clean while leaving the same race, and a *correct* fix changes a real
design trade-off the module's own docs cite on purpose (returning an owned
copy costs the leak-free, zero-allocation shape `borrowed()` was built for).
No reproduction exists today — nothing in the test suite drives `apply()`
concurrently with a vtable call — so there is also no red test to anchor a
fix against without first building one, which is its own design decision
(a fake concurrent harness, or a documented "EDS never actually calls these
concurrently with `apply()`" argument backed by reading EDS's own call
discipline). Recommended next step for whoever picks this up: first
determine from the EDS 3.52 source whether `EOAuth2Service` vtable calls and
`apply()` (driven by `insert_entries`'s GTK signal handlers) can actually
race on the same source in practice — if EDS's own threading model rules it
out, this becomes a KEEP with a documented reason instead of an IMPROVE.

#### Resolution 2026-08-20 (on opus, per the escalation at `0db0438`) — CONFIRMED and fixed

The recommended next step was taken: the EDS 3.52.3 sources (the installed
version — `dpkg -l` → `3.52.3-0ubuntu1.2`) were read rather than reasoned
about. EDS's threading model does **not** rule the race out, and the read
corrected this section's account of it in three ways.

**1. The racing writer in production is `set_property`, not `apply()`.**
Nothing outside `jmap-config/tests/backend.rs` calls `oauth2::apply` at all.
The production writer is `config_lookup::add_result`, which publishes
`[JMAP OAuth2]` as `EConfigLookupResult` string properties — and, more
importantly, `e-source.c`'s `source_parse_dbus_data` →
`source_load_from_key_file` → `g_object_set_property`, which re-runs every
`E_SOURCE_PARAM_SETTING` property whenever the registry pushes new source
data over D-Bus, on whatever thread the `GDBusProxy` notify lands on. That
is a more frequent and more certainly concurrent writer than the
GTK-signal-driven `apply()` this section hypothesised, not a less likely
one.

**2. EDS's own OAuth2 services avoid the hazard by never freeing what they
hand out — which is the fix.** `e-oauth2-service.c`'s five `const gchar *`
wrappers carry no transfer or threading annotation at all, and every EDS use
of the result copies it a few instructions later
(`e_oauth2_service_util_set_to_form`, `eos_create_soup_message`) with no
lock held. So no lock *can* cover the caller's use of the pointer: the only
workable contract is the lifetime `borrowed()`'s doc already claimed.
`e-oauth2-service-google.c` shows EDS keeping exactly that contract —
`eos_google_get_client_id` answers either a `static gchar glob_buff[128]` or
a value `eos_google_read_settings` caches via `g_object_set_data_full`
behind an `if (!value)` guard, i.e. written once and never replaced or freed
while the service lives.

**3. The "same contract as EDS's own extension accessors" defence was the
wrong comparison.** `e_source_authentication_get_host` and friends are
indeed lock-free, but EDS pairs each with a `dup_` variant taking
`e_source_extension_property_lock`. A vfunc with a fixed `const gchar *`
signature has no such escape hatch, which is why EDS's OAuth2 impls use
write-once storage instead.

**The fix**, therefore, adopts EDS's own discipline rather than adding a
lock that could not help: a `Fields` value is never freed once written.
`Fields::set` is now the single path both writing doors (`apply` and
`set_property`) go through; it performs no write at all when the value is
unchanged — keeping the existing allocation, and so any pointer already
handed out of it, exactly where it was, the same compare-then-skip
`source_set_property_from_key_file` already does one frame up — and moves a
replaced value into a `Fields::retired` vector dropped only in `finalize`.
Moving a `CString` moves the pointer, not the bytes, so retiring and any
later vector reallocation both leave an outstanding `const gchar *` valid.
The zero-allocation read shape `borrowed()` exists for is untouched. The
growth traded for this is bounded by the number of writes that actually
change a field — a human-paced event (an account's discovered OAuth 2.0
configuration changing), and one EDS itself already declines to perform when
the value did not differ.

**It was a real use-after-free, not a theoretical one.** The red test
`a_pointer_handed_out_before_a_changed_write_still_reads_its_original_bytes`
takes the pointer `get_client_id` would return, performs the changed
`set_property` write EDS's reload path performs, churns the allocator with
same-sized allocations, and reads the pointer back. Against the unfixed
code it returned `Some("XXXXXXXXXXXXX")` — the churn's own bytes, in the
reused heap block — instead of `Some("client-abc123")`. Two further tests
pin the pointer-stability half through both writing doors
(`rewriting_a_field_with_the_same_value_keeps_the_pointer_it_handed_out`,
`setting_a_property_to_the_value_it_already_has_keeps_the_pointer_it_handed_out`);
both failed on the unfixed code with two distinct addresses. All three are
in `jmap-config/tests/oauth2.rs`, and answer `docs/UNSAFE-AUDIT.md`'s
original complaint that this invariant was "not pinned by a test the way
most other invariants in this file are".

**One narrower thing deliberately left:** the retained `client_secret`
values now live until the extension is finalized rather than being freed at
the moment they are replaced. That is not a new exposure class — the same
secret is persisted to the account's `.source` file on disk by
`E_SOURCE_PARAM_SETTING`, which is the whole point of the property — so no
zeroization was added here; noting it so a future secrets-hygiene pass has
it recorded rather than having to rediscover it.

### Finding 4 — logged, not fixed: `insert_entries`'s asymmetric NULL guards

See "Question 3" above. Cosmetic; not scheduled.

## Overall verdict

The codebase's FFI surface is, per this audit, **sound** on all four named
questions: catch_unwind coverage is total (Track A6's own finding,
reconfirmed), transfer correctness and nullability came back clean across
five of six crate groups with one low-priority hardening gap and one
cosmetic inconsistency, `GCancellable` is honoured everywhere it should be,
and the one mechanical inconsistency found (three ungated vtable functions)
is fixed in this session. The one finding that is a genuine open question —
`oauth2.rs::borrowed`'s pointer lifetime — is a concurrency design question,
not a soundness bug proven to manifest, and is left for a session that can
either rule it out from EDS's own threading contract or design and TDD a
real fix; forcing a guess into this audit would risk exactly the
plausible-but-wrong outcome the night-shift escalation criteria exist to
avoid.

**Amended 2026-08-20.** That last sentence's caution was right, and the
session it deferred to has now run (on opus): Finding 3 was **not** merely a
design question. Reading the EDS 3.52.3 sources instead of reasoning about
them confirmed the race, identified a *different* and more frequent racing
writer than this audit had hypothesised (EDS's own D-Bus source reload, not
`apply()`), and a red test demonstrated the use-after-free concretely — it
read the churned heap block's bytes back through the handed-out pointer. See
Finding 3's "Resolution" subsection. So the audit's verdict now reads: sound
on all four named questions, with all three actionable findings (1, 2, 3)
fixed and one cosmetic one (4) unscheduled. The deferral cost nothing except
time, but the finding it deferred was a real memory-safety bug rather than
the "MEDIUM confidence, may be a KEEP" this table first recorded — worth
noting for how the next audit calibrates a lifetime argument nothing tests.
