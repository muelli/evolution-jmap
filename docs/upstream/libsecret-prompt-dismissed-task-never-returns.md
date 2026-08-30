# libsecret: `secret_service_real_prompt_async` never completes its `GTask` when a prompt is dismissed

Submit-ready. File at https://gitlab.gnome.org/GNOME/libsecret/-/issues
Everything below the horizontal rule is the issue body.

---

## Summary

When a Secret Service prompt is dismissed,
[`secret_prompt_perform_finish()`](https://gitlab.gnome.org/GNOME/libsecret/-/blob/98fc993200bedc925b6779a2998de1c3e58f0cad/libsecret/secret-prompt.c#L508)
returns `NULL` **without setting a `GError`**. Its own documented contract
allows exactly that.
[`on_real_prompt_completed()`](https://gitlab.gnome.org/GNOME/libsecret/-/blob/98fc993200bedc925b6779a2998de1c3e58f0cad/libsecret/secret-service.c#L320-L339)
passes that `NULL` straight to
[`g_task_return_error()`](https://gitlab.gnome.org/GNOME/glib/-/blob/2.80.0/gio/gtask.c#L2039-L2050),
which GLib rejects, so the `GTask` is never completed at all.

The callback of the async prompt operation therefore never runs. Any
synchronous caller above it blocks until something further up times out,
[`secret_service_prompt_sync()`](https://gitlab.gnome.org/GNOME/libsecret/-/blob/98fc993200bedc925b6779a2998de1c3e58f0cad/libsecret/secret-service.c#L1855)
included.

## Affected versions

Observed on libsecret 0.21.4 (Ubuntu 24.04, package `0.21.4-1build3`), with
GLib 2.80.0.

Still present on `main`, so this is not fixed. All libsecret permalinks below
are pinned to
[`98fc993`](https://gitlab.gnome.org/GNOME/libsecret/-/tree/98fc993200bedc925b6779a2998de1c3e58f0cad).
`secret-prompt.c` is byte-identical between 0.21.4 and that commit;
`secret-service.c` differs only by an offset of three lines, so
`on_real_prompt_completed()` is at 317-336 in 0.21.4 and 320-339 here.

## Symptom

Two GLib criticals from the affected process, in this order and in the same
second:

```
g_task_return_error: assertion 'error != NULL' failed
GTask secret_service_real_prompt_async (source object: 0x…, source tag: 0x…) finalized without ever returning (using g_task_return_*()).
```

The application then hangs rather than reporting a failure.

## Analysis

[`on_real_prompt_completed()`](https://gitlab.gnome.org/GNOME/libsecret/-/blob/98fc993200bedc925b6779a2998de1c3e58f0cad/libsecret/secret-service.c#L320-L339)
branches solely on whether the returned variant is `NULL`: non-`NULL` goes to
[`g_task_return_pointer()`](https://gitlab.gnome.org/GNOME/libsecret/-/blob/98fc993200bedc925b6779a2998de1c3e58f0cad/libsecret/secret-service.c#L332),
and everything else to
[`g_task_return_error()`](https://gitlab.gnome.org/GNOME/libsecret/-/blob/98fc993200bedc925b6779a2998de1c3e58f0cad/libsecret/secret-service.c#L336)
with whatever the local `error` holds, without checking that anything set it.

[`secret_prompt_perform_finish()`](https://gitlab.gnome.org/GNOME/libsecret/-/blob/98fc993200bedc925b6779a2998de1c3e58f0cad/libsecret/secret-prompt.c#L508)
has three `return NULL` paths, and **two of them leave `*error` untouched**:

- the [dismissed case](https://gitlab.gnome.org/GNOME/libsecret/-/blob/98fc993200bedc925b6779a2998de1c3e58f0cad/libsecret/secret-prompt.c#L526-L527),
  where the closure's result is `NULL`;
- the [unexpected-result-type case](https://gitlab.gnome.org/GNOME/libsecret/-/blob/98fc993200bedc925b6779a2998de1c3e58f0cad/libsecret/secret-prompt.c#L528-L533),
  which only issues a `g_warning()`.

Only the [`g_task_propagate_boolean()` failure](https://gitlab.gnome.org/GNOME/libsecret/-/blob/98fc993200bedc925b6779a2998de1c3e58f0cad/libsecret/secret-prompt.c#L520-L523)
sets an error.

This is deliberate, not an oversight in that function. Its own
[documented return contract](https://gitlab.gnome.org/GNOME/libsecret/-/blob/98fc993200bedc925b6779a2998de1c3e58f0cad/libsecret/secret-prompt.c#L504)
states it returns "%NULL if the prompt was dismissed or an error occurred",
and the same wording appears on
[`secret_prompt_perform_sync()`](https://gitlab.gnome.org/GNOME/libsecret/-/blob/98fc993200bedc925b6779a2998de1c3e58f0cad/libsecret/secret-prompt.c#L198).
A dismissed prompt is by design *not* a `GError`.

So "`NULL` with no error set" is a normal, expected, documented outcome, and
`on_real_prompt_completed()` treats it as impossible.

The consequence then follows from GLib's own code. All references are to GLib
2.80.0, the version this was observed on:

1. [`g_task_return_error()`](https://gitlab.gnome.org/GNOME/glib/-/blob/2.80.0/gio/gtask.c#L2039-L2050)
   guards `g_return_if_fail (error != NULL)` at gtask.c:2045, *above*
   `task->error = error` (2047) and `g_task_return()` (2049).
2. [`g_return_if_fail()`](https://gitlab.gnome.org/GNOME/glib/-/blob/2.80.0/glib/gmessages.h#L649-L660)
   logs and then returns from the enclosing function, so neither of those two
   statements is reached.
3. `task->ever_returned` is assigned in exactly one place in the file, inside
   [`g_task_return()`](https://gitlab.gnome.org/GNOME/glib/-/blob/2.80.0/gio/gtask.c#L1387-L1393),
   so it stays unset.
4. At finalization,
   [`if (!task->ever_returned)`](https://gitlab.gnome.org/GNOME/glib/-/blob/2.80.0/gio/gtask.c#L726-L745)
   emits the second critical.

That accounts for both observed messages and their order. The first is
generated at gtask.c:2045 specifically: `g_return_if_fail()` passes
`G_STRFUNC` and the stringified expression to `g_return_if_fail_warning()`,
which is why the logged text names `g_task_return_error` and `'error != NULL'`.

The task is therefore never completed, so every caller waiting on that
operation waits forever.

## How a prompt gets dismissed without an error

Any of these reach the same place:

- the user cancels an unlock prompt;
- no prompter can be shown, so the prompt is refused;
- the prompting peer disappears while the prompt is in flight. One way to
  arrange this is `gnome-keyring-daemon --replace`, replacing a daemon that
  has prompts outstanding.

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
  shape is the same: a libsecret task never returned, and a caller hanging
  indefinitely.
  https://bugs.launchpad.net/ubuntu/+source/gnome-control-center/+bug/2125590

## Suggested fix

In [`on_real_prompt_completed()`](https://gitlab.gnome.org/GNOME/libsecret/-/blob/98fc993200bedc925b6779a2998de1c3e58f0cad/libsecret/secret-service.c#L320-L339),
distinguish dismissal from failure:

- if `error` is set, `g_task_return_error()` as now;
- otherwise `g_task_return_pointer (task, NULL, NULL)`.

[`secret_service_real_prompt_finish()`](https://gitlab.gnome.org/GNOME/libsecret/-/blob/98fc993200bedc925b6779a2998de1c3e58f0cad/libsecret/secret-service.c#L362-L379)
already handles a `NULL` pointer correctly, propagating the pointer and
returning `NULL` when there is none. This therefore preserves the documented
"`NULL` means dismissed" semantics through the async path, without changing any
public contract.

If instead a dismissed prompt ought to be an error at this layer, then
`secret_prompt_perform_finish()` should set one, and its documented return
contract should change to match. Either way the current combination cannot be
correct: one function documents `NULL`-without-error as normal, and its caller
treats it as unreachable.

## Related issues

#75 and #113 are the same exactly-once `GTask` discipline failing in the
opposite direction: `g_task_return_error: assertion '!task->ever_returned'
failed`, a task returned *twice*, in
[`on_search_loaded()`](https://gitlab.gnome.org/GNOME/libsecret/-/blob/98fc993200bedc925b6779a2998de1c3e58f0cad/libsecret/secret-methods.c#L121)
from
[`search_load_item_async()`](https://gitlab.gnome.org/GNOME/libsecret/-/blob/98fc993200bedc925b6779a2998de1c3e58f0cad/libsecret/secret-methods.c#L150).
Different function and different outcome (a crash rather than a hang);
mentioned only because the shared underlying theme is async completion paths
that do not guarantee exactly one return.
