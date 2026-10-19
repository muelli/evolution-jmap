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
