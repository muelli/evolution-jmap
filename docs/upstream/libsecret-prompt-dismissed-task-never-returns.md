# libsecret: `secret_service_real_prompt_async` never completes its `GTask` when a prompt is dismissed

Submit-ready. File at https://gitlab.gnome.org/GNOME/libsecret/-/issues
Everything below the line is the issue body.

---

## Summary

When a Secret Service prompt is dismissed, `secret_prompt_perform_finish()`
returns `NULL` **without setting a `GError`** — which its own documented
contract allows. `on_real_prompt_completed()` passes that `NULL` straight to
`g_task_return_error()`, which GLib rejects, so the `GTask` is never completed
at all.

The callback of the async prompt operation therefore never runs, and any
synchronous caller above it blocks until something further up times out.

## Affected versions

Observed on libsecret 0.21.4 (Ubuntu 24.04, package `0.21.4-1build3`).

The code is unchanged on `main` as of 2026-08-29, so this is not fixed.

## Symptom

Two GLib criticals from the affected process, in this order and in the same
second:

```
g_task_return_error: assertion 'error != NULL' failed
GTask secret_service_real_prompt_async (source object: 0x…, source tag: 0x…) finalized without ever returning (using g_task_return_*()).
```

The application then hangs rather than reporting a failure.

## Analysis

`libsecret/secret-service.c`, `on_real_prompt_completed()` — lines 317-336 in
0.21.4, lines 320-339 on `main` — branches solely on whether the returned
variant is `NULL`. In the `NULL` branch it calls `g_task_return_error()` with
whatever the local `error` holds, without checking that anything was set.

`libsecret/secret-prompt.c`, `secret_prompt_perform_finish()` has three
`return NULL` paths, and **two of them leave `*error` untouched**:

- the dismissed case, where the closure's result is `NULL`
  (0.21.4, secret-prompt.c:526-527);
- the unexpected-result-type case, which only issues a `g_warning()`
  (secret-prompt.c:528-533).

Only the `g_task_propagate_boolean()` failure sets an error.

This is deliberate, not an oversight in that function. Its own documentation
states it returns "%NULL if the prompt was dismissed or an error occurred"
(secret-prompt.c:504). A dismissed prompt is by design *not* a `GError`, and
the same wording appears on `secret_prompt_perform_sync()`.

So "`NULL` with no error set" is a normal, expected, documented outcome, and
`on_real_prompt_completed()` treats it as impossible.

The consequence follows mechanically: `g_task_return_error()` fails its
`error != NULL` precondition and returns **without completing the task**. The
task is later finalized having never returned. Every caller waiting on that
operation waits forever, and `secret_service_prompt_sync()` and its callers
block.

## How a prompt gets dismissed without an error

Any of these reach the same place:

- the user cancels an unlock prompt;
- no prompter can be shown, so the prompt is refused;
- the prompting peer disappears while the prompt is in flight — for example
  `gnome-keyring-daemon --replace` replacing a daemon that has prompts
  outstanding.

## Reproduction

1. Start a session and let a client connect to `org.freedesktop.secrets`.
2. Cause a prompt to be dismissed rather than answered. Running
   `gnome-keyring-daemon --replace --unlock --components=secrets,pkcs11`
   while an unlock prompt is outstanding does it reliably, because the
   prompting daemon vanishes mid-prompt.
3. Observe the two criticals above in the client's log, and the client
   hanging instead of failing.

## Impact, with a concrete case

On a GNOME autologin session, three `evolution-data-server` processes
connected to the secret service and requested stored OAuth 2.0 tokens. Three
seconds later a keyring daemon replacement occurred while an unlock prompt was
in flight. The prompt's peer vanished, the task was never returned, and each
affected process was left unable to reach any secret service for the rest of
its lifetime.

Measured effects: two separate 25-second stalls, matching the GDBus default
timeout, and stored OAuth 2.0 tokens becoming unreadable, which surfaced to the
user as repeated re-authentication windows for accounts whose refresh tokens
were present and readable in an unlocked keyring the whole time. Restarting the
affected process cleared it immediately.

The stall is the part this bug owns: a dismissed prompt should have produced a
prompt failure, not a hang.

## Other occurrences

- `secret-tool` on Ubuntu 24.04 produced the identical pair of criticals in an
  unrelated workflow (GNOME Remote Desktop setup):
  https://gist.github.com/greyltc/7085bff8f2e728b60077b81329019828?permalink_comment_id=4819806
- Ubuntu bug 2125590 is the same "finalized without ever returning" class from
  `secret_service_async_initable_init_async`, hanging gnome-control-center
  inside `secret_password_store_sync()`. Its root cause was elsewhere (a
  keyring not exported on D-Bus, fixed in gnome-keyring), but the failure
  shape — libsecret task never returned, caller hangs indefinitely — is the
  same: https://bugs.launchpad.net/ubuntu/+source/gnome-control-center/+bug/2125590

## Suggested fix

In `on_real_prompt_completed()`, distinguish dismissal from failure:

- if `error` is set, `g_task_return_error()` as now;
- otherwise `g_task_return_pointer (task, NULL, NULL)`.

`secret_service_real_prompt_finish()` already handles a `NULL` pointer
correctly — it propagates the pointer and returns `NULL` when there is none —
so this preserves the documented "`NULL` means dismissed" semantics through the
async path without changing any public contract.

If instead a dismissed prompt ought to be an error at this layer, then
`secret_prompt_perform_finish()` should set one, and its documented return
contract should change to match. Either way the current combination cannot be
correct: one function documents `NULL`-without-error as normal, and its caller
treats it as unreachable.

## Related issues

#75 and #113 are the same exactly-once `GTask` discipline failing in the
opposite direction — `g_task_return_error: assertion '!task->ever_returned'
failed`, a task returned *twice*, in `on_search_loaded()`. Different function
and different outcome (a crash rather than a hang); mentioned only because the
shared underlying theme is async completion paths that do not guarantee exactly
one return.
