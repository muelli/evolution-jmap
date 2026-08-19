<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Unsafe reduction / idiom audit

Inventory of every `unsafe` block/fn/impl/trait in `rust/crates/**/src` (test
modules excluded — they exercise the same helpers under a harness, not a new
surface), 2026-08-19, per `docs/ROADMAP.md` Track A6. Complements Track A5
(FFI *soundness*: `catch_unwind` coverage, transfer-full/none correctness,
nullability, `GCancellable`) — this audit is about *reduction, containment,
and idiom*: is each `unsafe` block as small and as well-abstracted as it
could be, and where the same idiom is hand-copied across files, is there a
concrete, safer shared helper worth building. Audit only, per the roadmap
text — no code changes landed in this pass; findings are prioritized for a
follow-up session.

Method: four read-only research passes, one per crate group (foundational
FFI/subclassing machinery; the three EDS backend crates; `jmap-mail`;
`jmap-config` + the demo `example-module`), each inventorying every `unsafe`
site with its `// SAFETY:` reasoning and a tentative KEEP/IMPROVE/INVESTIGATE
tag. Cross-checked against the source for the highest-value cross-cutting
claims before writing them up here (see each pattern's "confirmed" note).

## Headline

**~80 files carry `unsafe`, several hundred individual blocks.** The
codebase is unusually disciplined: nearly every block has an accurate
`// SAFETY:` comment tied to a documented function-level contract, every
`extern "C"` vfunc/trampoline is wrapped in `jmap_backend_core::trampoline`'s
`catch_unwind`-based `guard`/`guard_bool`/`guard_ptr`/`guard_value`, and
GObject subclass registration goes through one shared, already-audited
abstraction (`jmap_backend_core::subclass::{ObjectSubclass, InterfaceImpl}`)
rather than being hand-rolled per type. The overwhelming majority of
individual findings are **KEEP** — already minimal, already documented,
nothing safer available without changing the C ABI they bridge.

The real yield of this audit is not "block X should be smaller" (few are)
but **six cross-cutting patterns where the same already-safe idiom is
hand-copied across multiple files/crates with no shared helper** — each
occurrence individually correct, but each also an independent chance for a
future edit to get the copy wrong. One exception (Pattern C, libical/GObject
reference counting with no RAII wrapper) is a genuine, if currently
contained, safety-relevant gap rather than pure duplication. There is also
one crate, `example-module`, that stands apart: it's an explicitly
out-of-`default-members` demo/reference module on an older Rust edition,
predates the shared `jmap_backend_core::subclass`/`trampoline` machinery, and
hand-rolls what the production crates no longer do — its issues are real but
low priority given its status.

One genuinely good finding: `jmap_backend_core::instance::Slot<T>`,
`jmap-mail`'s `Changes`/`FolderInfoChain`/`MessageCache`, and
`jmap-backend-collection`'s `populate.rs::Frozen` guard are all already
exactly the "pointer/state-owning newtype with `Drop`" idiom this audit
would otherwise recommend inventing — they're the templates the IMPROVE
items below should follow, not something to redo.

## Cross-cutting patterns (highest audit value)

### Pattern A — INVESTIGATE: zeroed-memory test doubles, 6 copies across 5 crates

```
jmap-backend-book/src/backend.rs:130
jmap-backend-cal/src/backend.rs:143
jmap-backend-collection/src/backend.rs:89
jmap-config/src/backend.rs:166
jmap-mail/src/store.rs:540
jmap-mail/src/transport.rs:198
```
(counts confirmed by direct grep across `crates/*/src/*.rs`, cutting across
what any single research pass could see on its own — each pass surfaced 2–3
of these independently)

Each does `Box::new(unsafe { MaybeUninit::zeroed().assume_init() })` to build
a `Self` outside the GObject type system for a test that needs to call one
narrow, documented-safe-on-zeroed-memory method (e.g. a pure function of the
struct's own fields, never a real vfunc or Camel/EDS call). Every site has a
long, careful safety comment naming exactly which method is safe to call on
the result and why (every field is pointer/int-sized, so all-zero is valid;
`Slot`'s empty state is its zero state). `jmap-backend-collection/backend.rs`'s
version is the most careful of the six, explicitly naming which vfunc
(`populate`) is *not* safe to call on a detached instance and why.

**Assessment:** each individual use looks sound, but this is the single
riskiest *idiom* in the tree — constructing an instance of a
GObject-ancestor `#[repr(C)]` struct by zeroing memory, bypassing
construction entirely, is exactly the kind of thing that stays safe only as
long as every future field addition to the wrapped C struct remains
pointer/int-sized and every call site remembers which methods are excluded.
None of the six are `#[cfg(test)]`-gated — they're `pub`/`pub(crate)` fns
reachable from ordinary (non-test) code, relying on doc comments rather than
the compiler to keep them test-only.

**IMPROVE:** two independent, additive fixes:
1. Gate each `detached()`/equivalent behind `#[cfg(test)]` (or a `testing`
   feature) so "never call this outside a test" is enforced, not just
   documented. Small, mechanical, per-crate.
   - **DONE 2026-08-19** — each of the six is now `#[cfg(feature =
     "testing")]`-gated; each crate gained a `testing` feature turned on for
     that crate's own `cargo test` builds via a self dev-dependency (`<crate>
     = { path = ".", features = ["testing"] }`), since every call site lives
     in that crate's own `tests/*.rs` and integration tests build the library
     without `cfg(test)`, so a plain `#[cfg(test)]` gate would not have
     reached them. See NIGHT-LOG "Track A6 Pattern A: compiler-enforced
     test-only `detached()`".
2. Hoist the zeroing itself into one `jmap_backend_core` helper, e.g.
   `pub unsafe fn zeroed_box<T>() -> Box<T>` with the safety contract written
   once ("every field of `T` must be valid at all-zero — verify this holds
   for the specific `T` before calling") instead of six near-identical
   copies of the same paragraph. **Still open.**

Rough effort: **~1–2 hours** (six call sites, one new helper, no behavior
change — the tests keep passing since the invariant doesn't move, only where
it's checked). Fix 1 above is done; fix 2 remains.

### Pattern B — IMPROVE: "check the GType, then cast" borrow helper, ~13 copies

Two families, same shape, different trust level:
- **Trusted/dispatched** (no type check — sound because Camel/EDS only
  dispatches a class's vfuncs on instances of that class): `folder.rs::JmapFolder::borrow`,
  `store.rs::JmapStore::borrow`, `transport.rs::JmapTransport::borrow`.
- **Checked** (`g_type_check_instance_is_a` before the cast, because the
  pointer arrives via an ordinary property/argument rather than vfunc
  dispatch): `folder.rs::parent_store`, `server.rs::network`,
  `envelope.rs::internet`, `summary.rs::JmapSummary::borrow`,
  `message_info.rs::JmapMessageInfo::borrow`, `subscribe.rs::borrow`,
  `transfer.rs::mailbox_of` — confirmed by grep: `g_type_check_instance_is_a`
  appears in exactly these 6 `jmap-mail` files (`folder.rs`, `envelope.rs`,
  `message_info.rs`, `server.rs`, `transfer.rs`, `summary.rs`).

`envelope.rs::internet` is the best-justified instance: here a failed type
check becomes a user-facing `EnvelopeError::NotInternet`, not just an
internal `None` — the check is load-bearing for correctness, not merely
defensive, and its doc comment is the right model for what a shared helper's
contract should say.

**IMPROVE:** one generic pair in `jmap-backend-core`, e.g.
`unsafe fn checked_borrow<T>(ptr: *mut impl CType, gtype: GType) -> Option<&T>`
and `unsafe fn dispatched_borrow<T>(ptr) -> Option<&T>`, collapsing ~10
independent reimplementations to 2 audited ones and making the
checked-vs-trusted choice an explicit, visible decision at each call site
rather than an implicit one buried in each copy.

Rough effort: **~2–3 hours** (mostly mechanical call-site updates once the
two helpers exist; touches 9 files across `jmap-mail`, so needs a careful
`cargo test -p jmap-mail` pass, not a rebuild-the-world one).

### Pattern C — IMPROVE (the one real safety-adjacent gap): no RAII wrapper for libical/GObject ref-counted pointers

Concentrated in **`jmap-backend-cal/src/marshal.rs`** — confirmed by grep:
15 manual `component_unref`/`g_object_unref` call sites in that one file
alone (`component_from_ical`, `icalendar_with_time_zones`, `holds_event`,
`find_master`, `icalendar_from_instances`, `take_event_time_zones`,
`take_referenced_time_zones`, `defines_time_zone`, `rename_time_zone`,
`referenced_tzids`) — plus the same "get owned ref → use → manually unref on
every exit path" shape recurring in `jmap-backend-collection` (`backend.rs::export`,
`fan_out.rs::apply_fanout`/`adopt`, `populate.rs::populate`) and, per the
`jmap-mail` pass, in ~10 more sites (`expunge.rs::is_deleted`,
`synchronize.rs::push_row`, `message.rs::listed_size`, `service.rs::name_of`/`attempt`,
`summary.rs::apply_message`, `message_info.rs::address_list`, `mime.rs::write_message`).

Every site checked in this audit unrefs correctly on every path — this is
not a live leak or use-after-free report — but correctness currently depends
on each site being individually re-verified by a human every time a new
early-return is added nearby, with no compiler help. `take_referenced_time_zones`
(cal marshal.rs) is the single densest instance: ~4 distinct
"owned pointer, conditionally freed on one of several exit paths" situations
in one 45-line function. Contrast with `populate.rs::Frozen` (a real `Drop`
guard already wrapping the freeze/thaw counter two lines away from one of
these manual-unref loops) and `jmap-mail`'s `Changes`/`FolderInfoChain`/`MessageCache` —
the codebase already knows this pattern and applies it in several places;
it's just not applied to GObject/libical pointers specifically.

**IMPROVE:** a small `struct Owned<T>(*mut T)` (or per-type
`OwnedComponent`/`OwnedSource`) with a `Drop` calling the right unref
function, `Deref`/`as_ptr()` for read access. Highest-value target:
`jmap-backend-cal/marshal.rs`'s timezone-handling cluster, since it's the
densest concentration of manual reference counting found in the whole audit
and the least amenable to "just read it carefully" review as it grows.

Rough effort: **~1 day** for the wrapper + `jmap-backend-cal/marshal.rs`
migration (needs re-deriving each function's ownership story as a type
rather than a comment, and the existing round-trip/fixture tests are the
acceptance suite); the `jmap-mail`/`jmap-backend-collection` sites are a
natural, smaller follow-up once the type exists.

### Pattern D — IMPROVE: "has_extension, then get_extension, then cast" idiom, ~10+ copies

`jmap-backend-collection/resource_id.rs::resource_id_of`,
`collection_source.rs::parts_of/user_of/server_of`,
`child_added.rs::follow_collection`, `mail_child.rs::mail_service_of/follow_server`,
and (same shape, foundational crate) `jmap-backend-core/source.rs::SourceConfig::from_source`
and `oauth2.rs::source_uses_oauth2` each independently re-derive: test
`e_source_has_extension` first (because `e_source_get_extension` *creates*
what it can't find), then fetch and cast. Every site's comment repeats the
same correctness point about creation-on-lookup. Contrast with
`child_source.rs::extension<T>()`, which *is* a shared helper — but for the
opposite (create-if-absent) case, so it doesn't serve these read-only sites.

**IMPROVE:** `fn extension_if_present<T>(source: *mut ESource, name: &CStr) -> Option<*mut T>`
in `jmap-backend-core`, used at all ~10+ sites across
`jmap-backend-core`/`jmap-backend-collection`. This is the highest-value
consolidation in the collection crate specifically, and the one this
audit's brief named directly ("ESource extension casting duplicated across
book/cal/collection with no shared helper").

- **DONE 2026-08-19 (the clean sites)** — `extension_if_present<T>` landed in
  `jmap-backend-core::marshal` (alongside `read_string`, the module's other
  type-agnostic FFI helper), and every call site whose shape was a plain
  single-source "guard then cast" now uses it:
  `jmap-backend-collection/resource_id.rs::resource_id_of`,
  `collection_source.rs::parts_of`/`user_of`/`server_of` (both its guards),
  `mail_child.rs::mail_service_of`, and `jmap-backend-core/oauth2.rs::
  source_uses_oauth2`. No behaviour change — every touched function's
  existing test suite (`resource_id.rs`, `collection_source.rs`,
  `oauth2.rs` in their respective `tests/`) stayed green unmodified,
  several of which specifically assert non-creation of the guarded
  extension. See NIGHT-LOG "Track A6 Pattern D: `extension_if_present`
  helper".
  **Still open, deliberately not folded into the same increment:**
  - `child_added.rs::follow_collection` and `mail_child.rs::follow_server`
    mix this "read if present" idiom with a genuine create-if-absent read on
    a *second* source in the same function; composing the helper there
    changes the call shape, not a 1:1 substitution.
  - `jmap-backend-core/source.rs::SourceConfig::from_source`'s
    `AUTHENTICATION`/`RESOURCE` reads skip the `has_extension` guard
    entirely today (unlike every sibling function's `SECURITY` guard, which
    *was* converted) — routing them through `extension_if_present` would
    fix a latent side effect (an account source silently gaining an empty
    `[Authentication]`/`[Resource]` group merely by being read), which is a
    behaviour decision nothing currently tests for, not a mechanical port.

Rough effort: **~2–3 hours** for the clean sites (delivered); the three
open items above are smaller follow-ups each, but need a decision or a
composed (non-mechanical) call shape rather than a straight port.

### Pattern E — IMPROVE (low priority): small repeated GError-builder / `fail()` helpers

`invalid_arg()` (`jmap-backend-book/ops.rs`, `jmap-backend-cal/ops.rs`,
`jmap-backend-collection/authenticate.rs::no_account_gerror` — same 3-line
body in three crates) and the `fail`/`fail_bool` sentinel-return wrapper
around `set_raw_gerror`, independently reimplemented in ~8 `jmap-mail`
modules (`manage`, `synchronize`, `refresh`, `append`, `send`, `subscribe`,
`folders`, `transfer`) plus `jmap-backend-book`/`jmap-backend-cal`'s `ops.rs`.
Each copy is 3–5 lines and already minimal-unsafe; the value here is fewer
near-identical bodies to keep in sync, not reduced risk.

**IMPROVE:** `invalid_arg_gerror(message: &str) -> *mut GError` and a
generic `fail<E>(error, failure: &E, to_gerror: impl FnOnce(&E) -> *mut GError) -> gboolean/*mut T`
in `jmap_backend_core::error`.

Rough effort: **~2 hours.** Lowest priority of the five IMPROVE patterns —
do it opportunistically alongside other work in these files rather than as
its own increment.

### Pattern F (positive — no action) — parent-class chain-up, and the panic-guard/subclass infrastructure

`g_type_class_peek(parent_type).cast::<ParentClass>().as_ref()` recurs
(`jmap-backend-book/backend.rs`, `jmap-backend-cal/backend.rs`,
`jmap-backend-collection/backend.rs`, `jmap-mail/service.rs`/`summary.rs`/`message_info.rs`,
`jmap-backend-core/subclass.rs::finalize_trampoline` itself) — worth a
one-line note as a *possible* future `parent_class<C>(parent_type) -> Option<&'static C>`
helper, but genuinely low priority: each occurrence is 2–3 lines, correctly
reasoned, and the type parameter would differ per call site anyway.

Separately, and this is the important positive finding: **every**
`extern "C"` vfunc/trampoline across all four crate groups is already routed
through `jmap_backend_core::trampoline::{guard, guard_bool, guard_ptr, guard_value}`,
and every GObject/interface registration goes through
`jmap_backend_core::subclass::{ObjectSubclass, InterfaceImpl, register_static, register_dynamic}`.
This is exactly the kind of centralization this audit exists to look for —
already done, consistently, everywhere except `example-module` (below). No
action needed; called out so a reader of this doc doesn't wonder why the
per-crate sections mostly say KEEP.

## `example-module` — out-of-`default-members` demo, lower priority but real gaps

Unlike every production crate, `example-module` has **no `// SAFETY:`
comments**, hand-rolls GObject type registration independently in two files
(`msg_composer_extension.rs`, `shell_view_extension.rs` — ~30 near-identical
lines of `g_type_query`/`GTypeInfo`/`g_type_module_register_type` in each,
duplicating what `jmap_backend_core::subclass::register_dynamic` already
does generically and more safely, deriving layout from `size_of::<T>()`
rather than copying the parent's query result plus an unstated
no-added-fields assumption), and has **no panic guard** around any
`extern "C"` callback — a panic in one would unwind into GObject/GTK, which
is UB. It also builds on Rust edition 2021 (not the workspace's 2024),
which is part of why its `unsafe fn` bodies compile without inner
`unsafe {}` blocks at all — `unsafe_op_in_unsafe_fn` isn't in force there.

One item is worth flagging beyond "adopt the shared machinery": `shell_view_extension.rs::get_private`
(`unsafe fn get_private(obj) -> &'static mut Private`) hands out an
unbounded-lifetime `&'static mut` from qdata with no lifetime tied to the
object — if its one caller (`shell_view_toggled_cb`) were ever reached
re-entrantly, this would produce aliasing mutable references. Low risk today
(single caller, GTK main-thread-only signal dispatch) but worth tightening
the signature if this module is ever promoted out of demo status.

**Given this crate needs EDS/Evolution UI headers to build, stays out of
`default-members` by design, and is explicitly a reference/demo module (not
shipped)**, none of this is prioritized work — recorded for completeness and
in case `example-module` is ever used as a template for a new extension
point, so its gaps aren't copied forward.

## Per-crate summary

- **`eds-sys`, `evo-sys`** (`src/lib.rs`, `build.rs`): no hand-written
  `unsafe` at all — each `lib.rs` is a doc comment plus
  `include!(bindgen-generated bindings.rs)`, and both `build.rs`es are pure
  safe build-script code. The entire raw-FFI surface here is mechanically
  regenerated per build and out of scope for a line audit by design (see the
  crate's own doc comment: keeping this the *one* place needing audit
  against the C ABI is the point).
- **`eds-sys/src/compat.rs`**: 8 small `unsafe fn`s, each with two
  `#[cfg]`-gated one-line arms for a pre-/post-EDS-version API rename, each
  independently safety-commented. KEEP across the board — the two-arm shape
  *is* the deduplication (one spelling difference, one place), not something
  to further consolidate.
- **`jmap-backend-core`**: the shared machinery crate — `subclass.rs`
  (registration + all 4 trampolines), `trampoline.rs` (the guard family),
  `marshal.rs` (typed out-param writers), `connect.rs` (`connect_with`, the
  one place vfunc-adjacent connect orchestration lives), `cancel.rs`
  (`GCancellable`↔`CancelFlag` bridge — one `transmute` at `cancel.rs:118-121`
  worth a small IMPROVE, single occurrence, low priority), `error.rs`
  (`GError` construction/marshalling, `set_raw_gerror` as the one write
  choke point), `instance.rs` (`Slot<T>` — already the "owning newtype with
  `Drop`" template), `source.rs` (`SourceConfig::from_source`, see Pattern
  D), `oauth2.rs` (`access_token`, careful read-before-free ordering), `i18n.rs`
  (gettext FFI, one documented process-lifetime pointer exception at
  `translate_static`). Overwhelmingly KEEP; contributes to Patterns C, D, F.
- **`jmap-backend-book`, `jmap-backend-cal`, `jmap-backend-collection`**
  (+ their thin `*-module` crates): structurally parallel (book/cal
  deliberately not merged — the module docs explain why and the reasoning
  holds up), each backend's 7-vfunc shape KEEP throughout, contributing to
  Patterns A, B, D, F. `jmap-backend-cal/marshal.rs` is the standout: the
  densest, most intricate unsafe surface in the whole audit (raw
  `ICalComponent`/`ICalProperty`/`ICalParameter` tree manipulation, no
  wrapper type) — individually correct today, the clearest Pattern C target.
  `jmap-backend-collection/backend.rs::drain` (`GList`→`Vec`, transfer-full,
  reused at 3 call sites) and `populate.rs::Frozen` (a real `Drop` guard) are
  positive examples already in the tree.
- **`jmap-mail`**: the largest crate (~9,800 lines, ~400 unsafe
  blocks/fns across 27 files) and, despite the size, the most consistent —
  nearly every block is a one-line FFI call with an accurate SAFETY comment,
  already at minimum scope. `cache.rs`'s `MessageCache` (mutex-guarded raw
  pointer, `Drop`-based unref) is the best-designed cluster in the entire
  audit and the right model to imitate. Contributes the bulk of Patterns
  A, B, C, E; also home to `folder_info.rs::FolderInfoChain` and
  `changes.rs::Changes`, both already-correct RAII wrappers.
- **`jmap-config`**: near-`jmap-backend-core`-level discipline — every
  extension-writer (`account.rs`, `mail.rs`, `oauth2.rs`) documents
  ownership per setter, every GObject/interface registration goes through
  the shared subclass machinery, `oauth2.rs::borrowed` is a good example of
  5 near-identical accessors delegating to 1 shared unsafe fn. One
  INVESTIGATE: `oauth2.rs::borrowed`'s "no lock needed against concurrent
  mutation" argument is plausible (matches EDS's own single-threaded
  extension-access convention) but not pinned by a test the way most other
  invariants in this file are — worth a second look, not urgent.
  `backend.rs::insert_entries` (the account-setup GTK page) is the single
  largest concentration of unsafe blocks in the crate (~25 in one function)
  but each is individually justified GTK widget wiring, not unsound;
  splitting it into smaller helpers would be cosmetic, not a safety
  improvement.

## Prioritized follow-up list

1. **Pattern A** (zeroed-memory test doubles, 6 sites, 5 crates) —
   `#[cfg(test)]`-gate + one shared helper. **~1–2 hours.** Do first: it's
   the one INVESTIGATE-tagged item with a concrete, cheap, compiler-enforced
   fix (test-only gating) available today.
2. **Pattern D** (`has_extension`-then-`get_extension` idiom, ~10+ sites) —
   one `extension_if_present<T>` helper in `jmap-backend-core`. **~2–3
   hours.** Named directly by this audit's own brief; clear highest-value
   consolidation in the collection crate.
   - **DONE 2026-08-19 for the clean sites** — see Pattern D's own section
     above for what landed and what is still open (`follow_collection`/
     `follow_server`'s composed shape, `SourceConfig::from_source`'s
     unguarded `AUTHENTICATION`/`RESOURCE` reads).
3. **Pattern B** (checked/trusted borrow helpers, ~13 sites across
   `jmap-mail`) — two generic helpers in `jmap-backend-core`. **~2–3 hours.**
4. **Pattern C** (no RAII wrapper for libical/GObject ref-counted
   pointers) — `Owned<T>` newtype, migrate `jmap-backend-cal/marshal.rs`'s
   timezone cluster first. **~1 day.** Highest safety value of the five, but
   the largest single increment — plan it as its own session with the
   existing round-trip/fixture tests as the acceptance bar, not folded into
   a quick pass.
5. **Pattern E** (`fail()`/`invalid_arg()` duplication) — **~2 hours**,
   lowest priority; fold into whichever of the above touches those files
   anyway rather than scheduling separately.

`example-module`'s gaps (no panic guards, hand-rolled registration,
`get_private`'s unbounded lifetime) are recorded above but intentionally
left off this priority list — it's a demo module outside `default-members`,
and none of the roadmap's real milestones depend on it.
