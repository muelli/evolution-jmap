<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Equivalent or out-of-scope mutants in `jmap-backend-core`

## `src/cancel.rs`

`CancelBridge::drop`'s guard, `self.handler == 0 || self.cancellable.is_null()`,
survives as `&&` in place of `||`. Every reachable state already behaves the
same under either operator: `CancelBridge::new` sets `handler` to `0` exactly
when `cancellable` is NULL or was already cancelled at connect time (GIO
invokes the callback synchronously and hands back no handler), so the two
conditions are never independently true. And `g_cancellable_disconnect`
itself is documented to do nothing when either argument is the one the guard
would have skipped on ("If cancellable is NULL or handler_id is 0 this
function does nothing", `Gio-2.0.gir`, `gio/gcancellable.c:616`). So calling
it unconditionally under the `&&` mutant is a safe no-op in every case the
`||` guard would have skipped, and skips it in exactly the same cases
otherwise. No test can distinguish the two without observing GIO's own
internal no-op, which is not this crate's contract to pin.

## `src/mime.rs`

`<impl Drop for Unwritable>::drop` survives replaced with `()`. The body
frees the `GError` the value owns with `g_error_free`; under the mutant,
an `Unwritable` built from `explained()` and never reported through
`into_gerror` would leak that `GError` instead. No test in this crate or in
`jmap-functional` can tell the two apart: `g_error_free` deallocates memory
and has no other observable effect, and this project runs no sanitizer or
leak checker (`ci/checks.sh` has no ASAN or valgrind pass) that would
surface the leak. Pinning this would need memory instrumentation this
project does not have, not a better test of this file's logic.

## `src/logging.rs`

`init` and `install` both survive replaced with `()`. Neither can be pinned
by a test in this crate's own test binary: `tracing` allows exactly one
global dispatcher per process, and several other files in this crate
(`connect.rs`, `resolver.rs`, `source.rs`, `trampoline.rs`) call
`evolution_jmap_tracing_capture::capture`, which claims that one slot for
itself on its first use so every test can get its own scoped capture
afterward. By the time any test here could check whether `install`'s
`try_init()` call actually ran, the slot is already taken, by
`jmap-tracing-capture`'s router if nothing else, regardless of whether
`install` itself ran or was skipped. Telling the two apart needs a fresh
process with no capture call ahead of it, which is what a module's real
entry point gives it and a shared test binary cannot.

## `src/weak.rs`

`<impl Drop for WeakBackend>::drop` survives replaced with `()`. The body
calls `g_weak_ref_clear`, which GLib's own documentation requires: a
`GWeakRef` registers its own address with the object it watches, and that
registration is only removed by `g_weak_ref_clear` (or by setting a new
target). Skipping it under the mutant does not stop the struct's `Box` from
being freed when `WeakBackend` is dropped (Rust's drop glue still runs on
the field after the empty `drop` body returns), so the object is left
pointing at memory that is already gone: if it is later disposed, GLib
writes into that freed allocation. No test in this crate can observe the
difference without actually exercising that write, which only shows up
under a sanitizer or leak checker (`ci/checks.sh` has none) or as
UB-dependent heap corruption that may or may not crash a given run. Pinning
this needs memory instrumentation this project does not have, not a
better test of this file's logic.

## `src/subclass.rs`

`ObjectSubclass::interfaces`'s default body survives replaced with `vec![]`.
The default already is `Vec::new()`, which `vec![]` builds too: the two
expressions construct the same empty `Vec`. No test can tell them apart
because there is nothing to tell apart.

`ObjectSubclass::class_init_types`'s default body survives replaced with
`vec![]` and with `vec![Default::default()]` (a one-element vec holding a
`GType` of 0). The default is `Vec::new()`, so the first is the same
equivalence as `interfaces` above. The second differs in value but not in
effect: `register`'s doc comment states the contract this type exists
under, "the list is the accessors' *effect*, not their values", and
`register` itself only ever binds the call to `_class_init_types`, an
underscore-prefixed binding it never reads. A type that needs a real
dependency registered overrides this method to call the dependency's own
`register_static` for the side effect (`Declaring` in this file's tests);
the default is only reached by a type with no such dependency, for which
one dummy `GType` in an otherwise-unread vec changes nothing observable.

## `src/i18n.rs`

Three mutants in `placeholder` time out rather than fail cleanly: the whole
body replaced with `Some((1, 0))`, `end += 1` replaced with `end *= 1`, and
`end + 2` replaced with `end - 2`. Each makes `substitute`'s loop advance
`rest` by zero bytes (or not at all) on every pass over a `%`, so the scan
over the template never terminates. `cargo-mutants` reports these as
`TIMEOUT`, not `MISSED`: the mutant does not pass silently, the test run is
killed because the behaviour diverged so badly it hung. No action needed
beyond this note; the two other `placeholder` mutants the same run first
found (`||` turned into `&&`, and `end + 2` turned into `end * 2`) were real
gaps and are now caught by
`jmap-backend-core::i18n::tests::digits_without_a_dollar_s_suffix_are_not_a_placeholder`
and `a_valid_placeholder_gives_its_position_and_byte_length`.

`N_`'s body survives replaced with `Box::leak(Box::new(Default::default()))`
only as an `Unviable` mutant: `&CStr` has no `Default`, so it does not
compile. No action needed.

## `src/resolver_thread.rs`

`lookup_first_service_target`'s body survives replaced with `None`, and the
`!targets.is_null()` guard before `g_resolver_free_targets` survives with
the `!` deleted. Both are only reachable with a *successful* SRV lookup: a
non-NULL `targets` list from `g_resolver_lookup_service`. Every lookup in
this crate's hermetic tests resolves `.invalid` domains (RFC 6761), which
never succeed, so `targets` is always NULL and both mutants are silent:
the full-body replacement skips work that would have produced the same
`None` anyway, and the deleted `!` only changes behaviour on the non-NULL
branch nothing here reaches. `jmap-backend-core/tests/resolver.rs` already
draws this exact line: its one test against a real SRV record,
`fastmail_publishes_its_jmap_host_via_srv`, is `#[ignore]`d because it
needs live DNS egress and asserts a third party's record, so cargo-mutants'
plain `cargo test` never runs it either. Closing this gap for real needs a
fake `GResolver` (a GObject subclass overriding `lookup_service`) to force
the success path hermetically, which is its own `unsafe`/vtable-correctness
undertaking, not a one-file mutation-testing session.

## `src/api_token.rs`

`source_uses_api_token` survives with its `e_source_authentication_get_type()`
call removed (a hand mutant: cargo-mutants skips an `unsafe fn`). The call is
insurance only. The contract asks for a valid `ESource`, and `ESource`'s class
init registers every built-in extension type, `ESourceAuthentication` too
(EDS 3.52 `e-source.c`, "Register built-in ESourceExtension types"). So the
type is always registered before the call, and no test can see a difference.
