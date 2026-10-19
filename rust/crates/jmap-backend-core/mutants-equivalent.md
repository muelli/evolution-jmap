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
