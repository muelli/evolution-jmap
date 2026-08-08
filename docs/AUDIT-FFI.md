# FFI audit — the unsafe core

An adversarial review of the code that crosses the C boundary: `eds-sys`,
`jmap-backend-core`, every `unsafe` block in `jmap-backend-book`,
`jmap-backend-cal` and `jmap-mail`, and the hand-written iCalendar and vCard
parsers against hostile input.

Session 1 — 2026-08-08, against EDS/Camel/libical-glib 3.52.3 on Debian trixie,
rustc 1.97.1, at `1c00a99`. Branch `audit/ffi`.

The threat model is the one the ROADMAP already implies: **the JMAP server is
not trusted**. It chooses every string, every object key, every count and every
`parentId` that reaches this code, and the code's output is fed to `EVCard`, to
libical and to Camel inside `evolution-addressbook-factory`,
`evolution-calendar-factory` and the mail process — each of which is also
serving the user's *other*, unrelated accounts. Local `.source` and Camel
settings files are semi-trusted: they are in the user's home but not
necessarily written by the user.

Everything below is either a finding with a test behind it or an explicit
statement that an area was looked at and found clean. Section 2 is the "looked
at, nothing found" register; absence of a finding there means it was checked,
not that it was skipped.

---

## 1. Findings

| | Severity | Where | Status |
|---|---|---|---|
| F1 | major | `jmap-vcard/src/syntax.rs` | fixed, `48fb68f` |
| F2 | major | `jmap-ical/src/syntax.rs` | fixed, `cd0a704` |
| F3 | major | `jmap-mail-sync/src/folder.rs` | fixed, `afece53` |
| F4 | minor | `jmap-ical/src/syntax.rs` | fixed, `04f3c90` |
| F5 | minor | `eds-sys/tests/layout.rs` | coverage added, `5bacac7` — no defect |
| F6 | info | `jmap-backend-{book,cal}/src/ops.rs` | recommendation |
| F7 | info | `jmap-backend-core/src/instance.rs` | assertion added, `48fb68f`/`cd0a704` |
| F8 | info | `jmap-mail/src/provider.rs` | pinned by test, `5bacac7` — no defect today |
| F9 | info | `jmap-backend-core/src/error.rs` | recommendation |
| F10 | info | `jmap-backend-cal/src/marshal.rs` | recommendation |

---

### F1 — major: a server-chosen JSContact map key injects vCard properties

**Where** `rust/crates/jmap-vcard/src/syntax.rs:337` (`quote_param`) via
`rust/crates/jmap-vcard/src/contact.rs:109` and `:122`
(`.with_param(X_JMAP_KEY, key)`).

**Why it is exploitable.** A `ContactCard`'s `emails` and `phones` are JSON
*objects*; their keys are strings the server picks with no constraint at all.
`card_to_vcard` round-trips each key through the `X-JMAP-KEY` parameter of the
`EMAIL`/`TEL` line it renders, so that a later save can put the entry back
under the same key. Parameter values do not pass through `escape` — and cannot:
RFC 2425 §5.8.2 gives a quoted parameter value no escape mechanism, so
`quote_param` could only drop the closing quote, which it did.

A CR or LF in a parameter value does not corrupt the parameter. It *ends the
content line*. Everything after it is a new content line, and the rendered card
goes straight to `e_contact_new_from_vcard` — i.e. into the user's address
book. Observed before the fix, from a key of
`e1\r\nFN:Mallory\r\nX-TAIL`:

```text
BEGIN:VCARD
VERSION:3.0
UID:C1
EMAIL;X-JMAP-KEY="e1
FN:Mallory
X-TAIL":vera@example.com
END:VCARD
```

`FN` is the name Evolution displays. A key carrying `END:VCARD` followed by
`BEGIN:VCARD` can start a second card instead. Nothing about this needs
`unsafe`; the boundary it crosses is a *format*, not a pointer.

**Evidence** `rust/crates/jmap-vcard/tests/hostile.rs` —
`a_crlf_in_a_map_key_cannot_add_a_property_to_the_card`,
`a_bare_lf_or_cr_in_a_map_key_is_stripped_too`,
`a_crlf_in_a_phone_map_key_cannot_add_a_property_either`,
`a_quote_in_a_map_key_cannot_open_a_parameter_of_its_own`, and
`a_crlf_in_a_value_is_still_escaped_rather_than_dropped` (the non-regression
half). The same payloads are then run all the way into `EContact` in
`rust/crates/jmap-backend-book/tests/hostile.rs` —
`a_map_key_cannot_give_a_nameless_contact_a_display_name` and
`a_map_key_cannot_terminate_the_card_early`. Three of these are red without the
fix.

**Fix** `48fb68f`. The strip is in `fold_into`, not in `quote_param`: that is
the one point every content line passes through — name, parameters and value
alike — so the guarantee is total and a new caller cannot reintroduce the hole
by choosing a different constructor. A value that means a line break still
spells it `\n`, which `escape` produces and `fold_into` leaves alone.

---

### F2 — major: server-supplied `duration`, `frequency` and `timeZone` inject iCalendar properties

**Where** `rust/crates/jmap-ical/src/syntax.rs:443` (`quote_param`) and `:48`
(`Property::raw`), via `rust/crates/jmap-ical/src/event.rs:106` (`TZID`),
`:114` (`DURATION`) and `:127` (`RRULE`).

**Why it is exploitable.** The calendar half of F1, with three vectors rather
than one, and two of them are values the mapping deliberately does *not*
escape because their punctuation is structure:

* `event.duration` → `Property::raw("DURATION", …)`, verbatim, **unvalidated**.
* `rule.frequency` → `format!("FREQ={}")` inside `Property::raw("RRULE", …)`,
  verbatim apart from an ASCII upcase, **unvalidated**.
* `event.time_zone` → `.with_param("TZID", zone)`, the parameter path, with the
  same no-escape-inside-quotes problem as F1.

`DTSTART` itself is safe: `to_ical_date_time` admits only digits in fixed-width
fields. `duration` and `frequency` have no such check. Observed before the fix:

```text
DTSTART;TZID="Europe/Berlin
DESCRIPTION:Injected Desc":20260115T130000
DURATION:PT1H
SUMMARY:Injected Summary
RRULE:FREQ=DAILY
LOCATION:INJECTED LOC
```

The rendered object goes to `i_cal_component_new_from_string` in
`load_component_sync`, so what libical stores in the user's calendar is what the
server wrote. `END:VEVENT` + `BEGIN:VEVENT` in the same place turns one
server-side event into two appointments with different uids.

**Evidence** `rust/crates/jmap-ical/tests/hostile.rs` —
`a_crlf_in_the_duration_cannot_add_a_property`,
`a_crlf_in_a_recurrence_frequency_cannot_add_a_property`,
`a_crlf_in_the_time_zone_cannot_add_a_property`,
`a_bare_lf_or_cr_is_stripped_as_well`, plus
`a_newline_in_a_text_value_is_still_escaped_rather_than_dropped`. Three are red
without the fix. `rust/crates/jmap-backend-cal/tests/hostile.rs` then asks
libical itself: `no_unescaped_value_can_add_a_property_to_the_component`
(exactly one `SUMMARY`, no `DESCRIPTION`, no `LOCATION`) and
`no_unescaped_value_can_close_the_event_and_open_another` (exactly one
`VEVENT`).

**Fix** `cd0a704`, in `fold_into`, for the reason given under F1.

---

### F3 — major: remote stack-overflow abort from a `parentId` chain

**Where** `rust/crates/jmap-mail-sync/src/folder.rs:65` —
`FolderInfo { …, children: Vec<FolderInfo> }`.

**Why it is exploitable.** `FolderTree::from_mailboxes` is careful about
everything a broken server sends — a `parentId` naming a mailbox it cannot see,
a `parentId` cycle, a duplicate id — and the build, the walk and `iter()` are
all iterative *specifically* so a server cannot choose a recursion depth. The
tree it returns is not: `FolderInfo` owns a `Vec<FolderInfo>`, so the compiler's
drop glue recurses once per level, and the level count is the length of a
`parentId` chain the server chose.

Measured: 20 000 mailboxes in one chain build and drop fine; **60 000 abort the
process** with `thread has overflowed its stack, fatal runtime error`. In a
Camel provider that is `evolution` — or an EDS mail process — serving every
other account the user has. There is no `unsafe` anywhere on the path and no
error return to catch: `catch_unwind` does not catch a stack overflow, so the
`guard` in `camel_provider_module_init` and every vfunc guard are irrelevant to
it. `Clone` on `FolderTree` recurses the same way.

`camel_folder_info_free` recurses over both `child` *and* `next`, so the C side
has the same shape of bound; `FolderInfoChain::from_tree` is iterative on our
side but the forest it hands over is what Camel then recurses into.

**Evidence** `rust/crates/jmap-mail-sync/tests/hostile.rs` —
`a_pathologically_deep_chain_neither_reshapes_into_a_stack_overflow_nor_hangs`
(100 000 mailboxes in one chain), with
`a_chain_within_the_limit_stays_one_chain` and
`a_chain_past_the_limit_is_cut_rather_than_dropped_or_refused` pinning the
shape. Without the fix the first test *aborts the test binary* rather than
failing — which is the whole point of the finding.

**Fix** `afece53`. A chain past `MAX_DEPTH` (64) is cut exactly the way `walk`
already cuts a `parentId` loop: the mailbox becomes top-level and its subtree
starts counting again. No mailbox is lost — a folder missing from the tree is
mail the user cannot reach — and bounding our depth also bounds Camel's
recursion over `child`. `jmap-mail`'s existing `a_deep_tree_…` test asserted
that 2 000 levels stayed 2 000 levels; it now asserts the intended shape (32
roots, 64 deep, all 2 000 folders present).

**Not fixed, documented:** the *breadth* bound. `camel_folder_info_free`
recursing over `next` means a flat account with 60 000 top-level mailboxes has
the same failure mode inside Camel. That is upstream's recursion and cannot be
fixed here; capping the number of folders would lose mail. Worth raising with
Camel.

---

### F4 — minor: stack-overflow abort from nested iCalendar components

**Where** `rust/crates/jmap-ical/src/syntax.rs:213` (`parse`), the tree it
returns, and `Component::write_into` at `:196`.

**Why it is wrong.** Same shape as F3 in a different crate. The parse *loop* is
iterative — `open: Vec<Component>` — so parsing is fine; the returned
`Component` tree is what recurses, in its drop glue and in `to_ics`. Measured:
20 000 levels fine, **50 000 abort**.

Reachability is narrower than F3 and that is why this is minor rather than
major: the text `syntax::parse` is given comes from
`i_cal_component_as_ical_string`, so libical has already parsed it (and would
hit its own recursion first) — the realistic route is an `.ics` a user imports
or opens, not the JMAP server. It is still an abort of a shared process from a
document, on a path with no `unsafe` in it.

**Evidence** `rust/crates/jmap-ical/tests/hostile.rs` —
`a_pathologically_nested_document_neither_parses_nor_crashes` (100 000 levels),
`a_document_nested_past_the_limit_is_refused_rather_than_parsed`,
`a_document_nested_up_to_the_limit_still_parses`; and at the C boundary,
`rust/crates/jmap-backend-cal/tests/hostile.rs` →
`a_pathologically_nested_object_is_an_error_not_an_abort`.

**Fix** `04f3c90`. `MAX_DEPTH = 32` and a new `ICalError::TooDeep`. RFC 5545's
deepest real nesting is three (`VCALENDAR` > `VEVENT` > `VALARM`, or >
`VTIMEZONE` > `STANDARD`).

The vCard grammar has no nesting, so `jmap-vcard` has no counterpart.

---

### F5 — minor: the factory class structs were subclassed but never layout-checked

**Where** `rust/crates/eds-sys/tests/layout.rs`.

**Why it mattered.** `tests/layout.rs` is the load-bearing test of the whole
FFI layer, and it vouched for every class struct the backends subclass *except*
the two factories. `JmapBookFactoryClass` and `JmapCalFactoryClass` are
subclassed exactly like the backends, and unlike them their parent half is
**written to**: `factory_name`, `backend_type`, and on the calendar side
`component_kind` all land at offsets bindgen computed from the header, while
what EDS reads them back at is decided by the compiled library. A disagreement
would be a `g_object_new(0)` per address book with no hint as to why — the exact
class of failure the file exists to catch.

The `EContact` → `EVCard` upcast in `jmap-backend-book/src/marshal.rs:114` was
the same kind of unasserted claim.

**Evidence** `rust/crates/eds-sys/tests/layout.rs` —
`backend_factory_layouts_match_the_gtype_system` (`EBackendFactory`,
`EBookBackendFactory`, `ECalBackendFactory`) and
`contact_layouts_match_the_gtype_system_and_a_contact_leads_with_its_vcard`.

**Result: CLEAN.** Both pass against 3.52.3. This is new coverage for a bet
that was already being made correctly, not a bug. `5bacac7`.

---

### F6 — info: an out-parameter is published before the rest of the outputs exist

**Where** `rust/crates/jmap-backend-book/src/ops.rs:74-77` and
`rust/crates/jmap-backend-cal/src/ops.rs:78-81`, same shape in both
`get_changes` bodies.

**Why it is fragile.** Both modules document the invariant EDS relies on —
*"on failure nothing is written and `error` is set instead, because EDS only
frees the outputs of a call that succeeded"* — and then write
`out_new_sync_tag` (a `g_strdup`, ownership transferred) *before* building the
lists. If anything between the two writes ever panics, `guard_bool` converts it
to `FALSE` with an error set, EDS frees nothing, and the sync tag leaks.

**Not a live bug.** The intervening code is `cstring_lossy`,
`e_book_meta_backend_info_new` and `g_slist_prepend`; none can panic, and a
Rust allocation failure aborts rather than unwinds. So the leak is currently
unreachable. It is recorded because the invariant is upheld by *what the code
happens to do*, not by its shape.

**Recommendation** build every output into locals first and publish them in one
run at the end, so the invariant is structural. No fix applied — this is a
refactor, not a defect.

---

### F7 — info: nothing checked that the connection is `Send + Sync`

**Where** `rust/crates/jmap-backend-core/src/instance.rs:55` (`Slot<T>`) and
the two backends' instance structs.

**Why it matters.** `JmapBookBackend` holds `Slot<RwLock<Option<BookSync>>>`,
and `with_connection` hands `&BookSync` to whichever thread EDS dispatched the
vfunc on — the `RwLock` is there precisely because EDS dispatches the read-only
vfuncs concurrently. The compiler never sees that sharing: the instance arrives
as a raw pointer and becomes a `&` by hand in `instance()`, whose lifetime is
unconstrained. So if `BookSync`, `Client` or the boxed `Transport` inside it
ever grew an `Rc` or a `RefCell`, nothing would complain and the result would be
a data race in someone else's process.

Checked by hand today: `Transport: Send + Sync + 'static` is a supertrait bound,
`Client` is `Box<dyn Transport>` + `Option<String>` + `AtomicU64` + plain data,
so both `BookSync` and `CalSync` are auto `Send + Sync`. **CLEAN**, but
unpinned.

A related, narrower gap in `Slot<T>` itself: `_owns: PhantomData<T>` makes
`Slot<T>: Sync` follow `T: Sync` alone, while `clear()` runs `T`'s destructor
on the calling thread and therefore wants `T: Send` too. `clear` is `unsafe`, so
the obligation is formally on the caller, and `Drop` needs `&mut self` (hence
`Slot<T>: Send`, hence `T: Send`) — so it is not reachable today. Recorded as a
latent soundness gap in a safety-critical primitive.

**Evidence** `the_connection_an_instance_holds_is_shareable_across_threads` in
`rust/crates/jmap-backend-book/tests/hostile.rs` (also asserting
`jmap_client::Client`) and in `rust/crates/jmap-backend-cal/tests/hostile.rs`.
A compile error rather than a comment. `48fb68f`, `cd0a704`.

---

### F8 — info: `&'static CamelProvider` aliases memory Camel holds mutably

**Where** `rust/crates/jmap-mail/src/provider.rs:157`.

**Why it is a claim worth checking.** `register()` returns
`&'static CamelProvider` — a *shared* Rust reference — to a leaked `Box` whose
raw pointer `camel_provider_register` keeps for the life of the process. That is
only within Rust's aliasing model while nothing on the C side ever writes there.
Camel's documented in-place work is translating `extra_conf` entries, which a
JMAP account leaves NULL.

**Evidence / result: CLEAN.**
`registering_a_provider_does_not_write_back_into_the_struct` in
`rust/crates/eds-sys/tests/camel.rs` compares the struct's bytes across
`camel_provider_register` on 3.52.3; they are identical, `priv_` included.
`5bacac7`.

**Recommendation** hand the pointer out as `*mut CamelProvider` rather than as
`&'static`, so the aliasing claim is not made at all. Not applied: the callers
would all need touching and the test now catches the failure mode.

Two smaller notes on the same file, neither a defect: the doc comment on
`authtypes` says "Empty rather than NULL-as-a-mistake" while the value is
`ptr::null_mut()` (which is what Camel wants — the comment means the *list* is
empty); and `unsafe impl Send/Sync for Registered` is justified as written
(published once under a `OnceLock`, never written after, never freed).

---

### F9 — info: `set_raw_gerror` overwrites and leaks an already-set `GError` in release

**Where** `rust/crates/jmap-backend-core/src/error.rs:85-94`.

**Why it is fragile.** The already-set case is a `debug_assert!`, so in a
release build the previous `GError` is silently overwritten and leaked. The path
that would reach it is a vfunc that reports an error and *then* panics:
`guard_bool` calls `set_raw_gerror(error, internal_error(…))` unconditionally.
No current body does that — every `fail*` helper returns immediately — so this
is not a live bug.

**Recommendation** make the non-debug path defensive: if `*dest` is already
non-NULL, free the *new* error and keep the first, which is the one closer to
the cause, and log a critical. Cheap and removes the class. Not applied: it
changes behaviour on a path that is currently unreachable, and the audit's
mandate is to fix clear-cut bugs.

---

### F10 — info: a save with no master instance is refused

**Where** `rust/crates/jmap-backend-cal/src/marshal.rs:190`
(`icalendar_from_instances`) and `find_master` at `:249`.

**Why it is worth recording.** `save_component_sync` is handed every instance
of one uid; the master is identified by the *absence* of `RECURRENCE-ID`, and a
set with no master is refused with `E_CLIENT_ERROR_INVALID_ARG`. That is the
right call given the mapping — `jmap-ical` does not cover
`recurrenceOverrides`, so writing a moved occurrence as if it were the series
would corrupt the user's recurrence — and it is deliberate and documented in
the code.

It is nonetheless a legal call shape EDS can make. `ECalMetaBackend` normally
includes the master when it modifies a single occurrence, so the refusal should
not fire in practice, but "should not" is doing work here and the symptom would
be an edit Evolution reports as failed for a reason the user cannot act on.

**Recommendation** revisit when `recurrenceOverrides` is mapped; until then
this is the honest behaviour. No code change.

---

## 2. Audited and CLEAN

Recorded so that "no finding" is distinguishable from "not looked at".

### 2.1 Struct layouts vs the installed headers

`eds-sys` generates bindings at build time and `tests/layout.rs` compares every
classed type's `size_of` against `g_type_query`. After `5bacac7` the covered
set is: `EBackend`, `ESource`, `EBookBackend`, `EBookMetaBackend`, `EBookCache`,
`ECalBackend`, `ECalMetaBackend`, `ECalCache`, `EBackendFactory`,
`EBookBackendFactory`, `ECalBackendFactory`, `EVCard`, `EContact`,
`ICalComponent`, `ECalComponent`, `CamelService`, `CamelStore`,
`CamelOfflineStore`, `CamelTransport`, `CamelSession`, `CamelSettings`,
`CamelStoreSettings`, `CamelOfflineSettings`, `CamelFolder`. All match.

What has no `g_type_query` answer, and how each is pinned instead:

* `CamelProvider` — boxed type, sizes report 0 (pinned by
  `a_provider_is_a_boxed_type_so_gtype_knows_nothing_of_its_size`). Pinned by a
  register/`camel_provider_get` round trip that reads four fields back, plus the
  `object_types` array length against `CAMEL_NUM_PROVIDER_TYPES`.
* `CamelFolderInfo` — plain struct. Pinned by `a_fresh_folder_info_is_zeroed`
  and `folder_info_names_survive_a_g_strdup_and_a_free`, which read every field
  back after a Camel allocation.
* `EBookMetaBackendInfo` / `ECalMetaBackendInfo` — plain structs allocated by
  EDS. Pinned by the `_info_new` → read-back-all-fields → `_info_free` round
  trips in `jmap-backend-{book,cal}/tests/marshal.rs`.
* `CamelNetworkSettings` — an interface, so no instance or class size exists.
  Pinned by `no_stock_camel_settings_class_carries_the_network_properties` and
  by the property round trips in `jmap-mail/tests/settings.rs`.
* `ICalProperty` — only ever `g_object_unref`ed; no field is read and no size
  is assumed.
* The `ESource*` extensions — read only through accessor functions, never by
  field.
* Individual *vfunc slot offsets* inside `EBookMetaBackendClass` /
  `ECalMetaBackendClass` are not asserted directly; the class *sizes* are, and
  `jmap-backend-{book,cal}/tests/backend.rs` dispatch every call through the
  class struct rather than at the Rust functions, which exercises the slots.
* `GObject`, `GError`, `GCancellable` and friends come from the gtk-rs sys
  crates, not from this bindgen run;
  `glib_types_are_the_gtk_rs_ones_not_regenerated_copies` pins that the
  blocklist is doing its job (a regenerated copy would have the right layout
  and the wrong identity).

The blocklist reasoning in `build.rs` — blocking both `G[A-Z].*` and the
`_G[A-Z].*` tag structs, because blocking only the typedef makes bindgen emit
the tag as a second incompatible `GObject` — is correct and is the subtlest
thing in the crate.

### 2.2 vfunc trampolines and panic paths

Every `extern "C"` function in the four crates was enumerated and traced to a
guard:

| entry point | guard |
|---|---|
| `class_init_trampoline`, `instance_init_trampoline`, `finalize_trampoline` | `guard` |
| `e_module_load` (book, cal) | `guard` |
| `e_module_unload` (book, cal) | none — empty body |
| `connect_sync`, `disconnect_sync` (book, cal) | `guard_bool` |
| `list_existing_sync`, `get_changes_sync`, `load_*_sync`, `save_*_sync`, `remove_*_sync` (book, cal) | `guard_bool`, via `with_connection` |
| `camel_provider_module_init` | `guard` |
| `CamelJmapSettings::{set,get}_property` | `guard` |
| `on_cancelled`, `destroy_flag` | none — an `AtomicBool` store and an `Arc` drop, neither of which can panic |

**Coverage is complete.** No unwind can reach C. Confirmed too that nothing
sets `panic = "abort"` in `rust/Cargo.toml`, the crate manifests, `cmake/` or
the workflows — the release profile only sets `codegen-units`, `strip` and
`incremental` — so `catch_unwind` genuinely catches rather than being
decorative. (A stack overflow is *not* an unwind; see F3 and F4.)

Other checks on this area:

* `finalize_trampoline` chains up **outside** the guard and via
  `g_type_class_peek(T::parent_type())` rather than
  `g_type_class_peek_parent(instance class)` — the latter would recurse into
  this same trampoline for a further subclass. Both choices are right and both
  are commented.
* `class_init_trampoline` installs `finalize` *before* running `T::class_init`,
  so a subclass can still take the slot.
* `register()` resolves `parent_type()` and `interfaces()` before taking the
  `REGISTRATION` mutex, avoiding a deadlock when a Rust-declared hierarchy
  bootstraps itself, and treats a poisoned lock as usable. The static path is
  idempotent via `g_type_from_name`; the dynamic path re-registers on every
  load, which is what `GTypeModule` requires.
* Interfaces are added between registration and returning the `GType`, so
  `g_object_class_override_property` in `class_init` can find the interface's
  properties. The reasoning in the doc comment is correct.
* NULL handling: `instance()` returns `Option` for a NULL instance;
  `connect_with` handles a NULL `ESource` rather than dereferencing it;
  `password()` handles the NULL `ENamedParameters` EDS passes before it has
  asked libsecret; `CancelBridge::new` handles a NULL `GCancellable`; every
  out-parameter writer skips NULL.
* Arguments EDS may legally pass that the code rejects: an empty `last_sync_tag`
  degrades to a full list rather than an error (correct); an empty `uid` on
  load/remove is an `INVALID_ARG` (EDS never sends one); `overwrite_existing`
  with no uid is refused rather than silently duplicating the user's data
  (correct, and the alternative is worse). The one genuinely arguable case is
  F10.
* `guard_ptr` is defined and tested but unused in production — the vfuncs this
  layer overrides all return `gboolean`. Harmless; it exists for
  `load_contact_sync`-shaped signatures a later milestone may need.

### 2.3 GObject memory and string ownership

Every C function called from the four crates (82 of them) was resolved against
the installed `.gir` files and its `transfer-ownership` annotation compared with
what the Rust does. **All 82 agree.** The ones that would have been bugs:

* `i_cal_component_as_ical_string` — `transfer full`; freed with `g_free` in
  `take_string`. ✔
* `i_cal_component_get_uid` — `transfer none`; borrowed, not freed. ✔
* `i_cal_component_{get_first_component,get_first_property,clone}` —
  `transfer full` (of the libical-glib *wrapper*); each is `g_object_unref`ed,
  including the deliberate unref-then-test-the-pointer in `holds_event`. ✔
* `i_cal_component_take_component` — the `child` parameter is `transfer full`,
  and it is given `i_cal_component_clone(master)` rather than the borrowed
  master. ✔
* `e_cal_component_get_icalcomponent` — `transfer none`; `find_master` returns
  it borrowed and documents that. ✔
* `e_vcard_to_string`, `camel_network_settings_dup_*` — `transfer full`;
  `g_free`d after copying. ✔
* `e_contact_get_const`, `e_named_parameters_get`,
  `e_source_authentication_get_*`, `e_source_resource_get_identity` —
  `transfer none`; copied, not freed. ✔
* `g_value_take_string` — takes ownership of its argument, which is exactly the
  `dup_` accessor's result. ✔
* `e_{book,cal}_meta_backend_info_new` — copies all four strings, so the
  `CString`s may be temporaries. ✔
* `g_strdup` for `CamelFolderInfo`'s names, so `camel_folder_info_free` frees a
  GLib allocation and not a Rust one — the specific hazard
  `folder_info_names_survive_a_g_strdup_and_a_free` exists for. ✔

Also checked: no floating references are involved (nothing here is a
`GInitiallyUnowned`); `Slot` is the only place a Rust destructor lives inside
GObject-allocated memory, and both backends clear it in `finalize`;
`FolderInfoChain` is the single owner of a `CamelFolderInfo` forest and
`into_raw` forgets `self`, so the `Drop` and the hand-over cannot both run;
`FolderInfoChain::from_tree` cannot fail part-way (the only fallible conversion,
a NUL in a name, is resolved by rewriting the name to U+FFFD rather than by
returning an error), so there is no half-built forest with no owner.

GError paths: `to_gerror` allocates and every caller either transfers it through
a `GError **` or frees it (`set_raw_gerror` frees when `dest` is NULL, which is
the GLib convention for "the caller does not want the error"). The one
fragility is F9. `cstring_lossy` truncating at an interior NUL rather than
panicking is the right call for a server-supplied string.

### 2.4 Threading

The three `unsafe impl Send`/`Sync` in production code:

* `jmap-mail/src/provider.rs:80-81`, `Registered(*mut CamelProvider)` —
  justified: written once under a `OnceLock`, never mutated afterwards (pinned
  by F8's test), never freed. ✔
* the two in `tests/factory.rs` (`Loaded`) are test scaffolding.

`example-module`'s four are out of scope (it is the Red Hat wiki example, LGPL,
not part of the JMAP backends).

Who calls what: EDS dispatches an `E*MetaBackend`'s sync vfuncs from the
factory's thread pool, so `list_existing_sync`, `load_*_sync` and friends can
overlap; `connect_sync`/`disconnect_sync` replace the connection. That is what
the `RwLock` is for, and the read/write split matches. Poisoned locks are
recovered from rather than propagated, which is right — a panic elsewhere does
not corrupt a `BookSync`. `on_cancelled` can fire from *any* thread, which is
why the flag is an `Arc<AtomicBool>`; `CancelBridge`'s scope is one vfunc call,
inside EDS's guarantee that the `GCancellable` outlives it. The unpinned part of
this area was F7.

### 2.5 Integer conversions

* `subclass::register` — `u16::try_from(size_of::<Class/Instance>())` for
  `GTypeInfo`'s `guint16` fields, panicking on overflow rather than truncating.
  The panic is inside a guard on every path that reaches it (`e_module_load`,
  `class_init`, `camel_provider_module_init`). Actual sizes are a few hundred
  bytes. ✔
* `folder_info::count` — `i32::try_from(u32).unwrap_or(i32::MAX)`, saturating,
  because Camel uses *negative* counts for "not known yet" and a truncated
  count with the top bit set would read as unknown rather than as large. ✔ The
  reasoning here is better than the usual `as` cast.
* `folder::saturate` — `u32::try_from(u64).unwrap_or(u32::MAX)`, the first half
  of the same argument. ✔
* `settings::set_property` — `g_value_get_uint(value) as u16`. Truncating in
  isolation, but the interface's pspec is `g_param_spec_uint(…, 0, G_MAXUINT16,
  …)` and `g_object_set_property` runs `g_param_value_validate` against the
  redirect target before calling the setter, so an out-of-range value never
  arrives. ✔ (Recorded rather than changed: the narrowing is Camel's own
  accessor signature.)
* `settings::get_property` — `camel_network_settings_get_security_method(…) as
  i32` for `g_value_set_enum`, which takes `gint`. Widening. ✔
* `e_source_authentication_get_port` returns `guint16`, matching `origin`'s
  `port: u16`; 0 means "not set" on both the keyfile and the Camel side. ✔
* No `usize`→`u32`/`i32` cast anywhere reads a length into a C field without a
  `try_from`.

### 2.6 Security rules from `docs/ROADMAP.md`

* **TLS by default.** `SourceConfig::from_source` asks
  `e_source_has_extension(E_SOURCE_EXTENSION_SECURITY)` *before* any
  `e_source_get_extension` (which would create the extension) and treats a
  missing `[Security]` group as secure. This is the subtle one:
  `ESourceSecurity:secure` defaults to FALSE, so an unconditional read would
  silently downgrade every hand-written account. ✔ On the Camel side the
  interface default is `CAMEL_NETWORK_SECURITY_METHOD_NONE` (= 0, pinned by
  `the_security_method_that_means_plaintext_is_the_zero_one`), and
  `jmap-mail/tests/settings.rs` pins that the property overrides make the
  applied default TLS. ✔
* **Plaintext to loopback only.** `source::origin` refuses `!secure &&
  !is_loopback(host)` with `E_CLIENT_ERROR_TLS_NOT_AVAILABLE`, and both the EDS
  and the Camel path go through that one function rather than through two
  copies. `is_loopback` accepts `localhost`, `localhost.localdomain` and
  loopback IPv4/IPv6 literals only; the near-misses that matter
  (`localhost.example.com`, `notlocalhost`, `127.0.0.1.example.com`, `0.0.0.0`,
  `::2`) were already tested. Four more spellings that *do* reach 127.0.0.1
  through a resolver were probed during this audit — `::ffff:127.0.0.1`,
  `2130706433`, `0177.0.0.1` and `127.1` — and none is treated as loopback, so
  each fails **closed**: plaintext is refused and TLS required. That is the safe
  direction, and `only_loopback_addresses_count_as_local` now pins it so that
  loosening the check has to be deliberate. ✔
* **Host validation before concatenation.** `authority()`/`is_bare_host_name`
  admit only ASCII alphanumerics, `.` and `-`, reject a leading `.`/`-` and any
  `..`, and bracket IPv6 literals. A host is validated *before* the TLS check
  and the validated string is the one used, so there is no check-then-use gap.
  On the Camel side `dup_host_ensure_ascii` is read *before* validation, so the
  checked string and the sent string are again the same one. ✔
* **Credentials only via `ESourceAuthentication`.** No credential is read from
  a `.source` keyfile or a Camel settings object anywhere:
  `marshal::password` reads `E_SOURCE_CREDENTIAL_PASSWORD` out of the
  `ENamedParameters` EDS filled from libsecret, and that is the only source.
  `SourceConfig` and `ServerConfig` deliberately have no password field. The
  provider's `CAMEL_URL_ALLOW_PASSWORD` is a URL *capability* flag, not a place
  a password is taken from. ✔
* An empty stored password reads as *present* rather than absent, which stops a
  prompt loop; a source that names a user with no password yet fails with
  `CredentialsRequired` before anything goes on the wire, so a password is
  never sent to a server the account has not been told to trust. ✔
* `connect_sync` writes `out_auth_result` on **every** path, and only a 401
  produces `REJECTED` (the one value that makes Evolution discard the stored
  password). ✔ Pinned by
  `only_a_401_makes_evolution_ask_for_the_password_again`.
* No regression: the security-relevant behaviour above is unchanged by this
  session's commits.

### 2.7 Parser robustness beyond F1–F4

Fuzz-style probing of both hand-written layers with malformed, oversized,
escape-abusing and truncated input found nothing further:

* Unterminated `BEGIN`, mismatched `END`, content after `END:VCALENDAR`, a line
  with no colon, a parameter with no `=`: all produce errors, none panic.
* `component_name` uses `line.get(..len)` so a multi-byte first character
  returns `None` instead of slicing off a char boundary. `parse_line` slices at
  a `:` found by `char_indices`, so `&value[1..]` is always on a boundary. No
  panicking index or slice in either module.
* Escape abuse: a trailing lone `\` unescapes to `\`; an unknown escape stands
  for the character itself; `split_unescaped` tracks the backslash state, so
  `\,` does not split. An unbalanced `"` in a parameter leaves `quoted` true to
  end of line, which means `find_unquoted` never finds the `:` and the whole
  line is rejected as `Malformed` — a refusal rather than a half-parse, which
  is the right way round.
* Multi-byte input in every position: a property name, a parameter name, a
  folded continuation and a truncated `BEGIN` keyword all parse or error
  cleanly; nothing slices off a char boundary.
* Oversized: measured 1 / 4 / 16 MiB single values through emit → parse →
  unescape at 50 / 199 / 792 ms of emit and 7 / 30 / 120 ms of parse — linear,
  no quadratic behaviour. `fold_into` is one pass, `unfold` is one pass plus one
  `replace`, and `unescape` is one pass.
* Interior NUL: handled at both boundaries —
  `folder_info::c_string` rewrites a NUL in a mailbox name to U+FFFD (rather
  than truncating `Work\0Secret` to a second indistinguishable `Work`), and
  `cstring_lossy` truncates a NUL in an error message. Both are already tested.
* The `entry_key` fallback (`(1..).map(…).find(…)`) terminates for any finite
  map. ✔

---

## 3. Notes for the calcard migration

`docs/ROADMAP.md`'s standing directive replaces both hand-written text layers
with `calcard`. Three things from this audit should survive the swap:

1. F1 and F2 are properties of the *emitter*, not of the mapping. Whatever
   `calcard` does, the tests in `jmap-vcard/tests/hostile.rs`,
   `jmap-ical/tests/hostile.rs` and the two backends' `tests/hostile.rs` are the
   acceptance criteria: a server-chosen string must not be able to add a
   content line. Keep them and point them at the new emitter.
2. F4's depth cap is a property of the *parser's return value*, and a
   `calcard` tree with an owning `Vec` of children has the same drop-glue
   recursion. Check it before assuming the cap can go.
3. F3 is in `jmap-mail-sync` and is untouched by the migration.

---

AUDIT COMPLETE
