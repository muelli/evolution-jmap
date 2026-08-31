<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# DRAFT, not filed

Target: `GNOME/glib`, issue. Measured against GLib/GIO 2.80.0 (Ubuntu 24.04)
on 2026-08-31; the mechanism was verified unchanged in `main` by source
inspection the same day. Found while building a JMAP SRV bootstrap lookup
(`_jmap._tcp`) for an Evolution plugin. Filed only after a maintainer has
read it, same rule as the other drafts in this directory.

---

## Synchronous GResolver lookups accumulate one GTask + completion idle per call in a main context the caller never iterates

### Summary

Every synchronous `GResolver` lookup (`g_resolver_lookup_by_name()`,
`g_resolver_lookup_service()`, `g_resolver_lookup_records()`, ...) leaves
behind one `GTask`, one idle `GSource`, its callback closure and its name
string, attached to the calling thread's thread-default main context. If that
context is never iterated, which is the normal situation for a program or
thread using only the synchronous API, the memory accumulates without bound:
about 0.6 kB RSS per call, plus a further ~0.4 kB per call held for 30 s
windows by the default lookup timeout. A process that resolves in a loop
grows at ~1 kB per lookup, linearly, forever.

Nothing is "definitely lost" in the valgrind sense; everything stays
reachable from the never-dispatched idle. That is why the classic leak
tooling shows a clean report while RSS climbs.

### Reproduction, measured

`srv-leak.c` (attached below) resolves the same domain N times through the
documented, reference-balanced sync sequence and prints its own `VmRSS`:

| run | rate |
|---|---|
| `srv-leak srv example.com 3000` (SRV, not-found path) | **0.97 kB/call** |
| `srv-leak srv fastmail.com 2000` (SRV, found path) | **0.98 kB/call** |
| `srv-leak name example.com 3000` (`lookup_by_name` control) | **1.04 kB/call** |
| same, with `MALLOC_ARENA_MAX=1` | unchanged, so not an allocator artifact |
| `srv-leak srv example.com 3000 0` (resolver `timeout` property set to 0) | **0.60 kB/call** |
| 2000 iterations, then 45 s idle (default 30 s timeout) | RSS never returns |

Both the SRV and the plain-name paths leak, at the same rate, because the
mechanism lives in the shared task plumbing, not in the DNS code.

valgrind (`--show-leak-kinds=all`, GLib 2.80.0):

* definitely lost: **0 bytes**.
* still reachable: **123,932 bytes after 100 iterations, 540,936 after 600**,
  i.e. 834 bytes per call, in records whose block counts equal the iteration
  count. With `timeout=0` the still-reachable per-call records remain and
  total 519 bytes per call.

The per-iteration records (one block per lookup, N blocks after N lookups):

```
g_idle_source_new           <- gtask.c: g_task_return's completion idle
g_source_set_callback       <- via g_task_attach_source, from the worker thread
g_strdup (source name)      <- "[gio] ... complete_in_idle_cb"
g_malloc0 184-byte block    <- the GTask instance itself
g_strdup                    <- under g_resolver_lookup_service (task name)
```

and, only when the resolver `timeout` property is nonzero (default 30000 ms):

```
g_timeout_source_new        <- gthreadedresolver.c:1531 (2.80.0)
g_source_set_callback       <- gthreadedresolver.c:1533
```

### Mechanism

File/line references are GLib 2.80.0; the same code is present in `main` as
of 2026-08-31 at shifted line numbers.

1. The synchronous vfuncs create a `GTask` with a NULL callback in the
   calling thread's thread-default context and hand the work to the
   resolver's own `GThreadPool`
   (`gio/gthreadedresolver.c`: `lookup_by_name()` at 348,
   `run_task_in_thread_pool_sync()` at 1553, `g_thread_pool_push()` at 1524).
2. The worker computes the result and calls `g_task_return_pointer()`. The
   task is not `synchronous` (that flag is private to
   `g_task_run_in_thread_sync()`) and not threaded, so `g_task_return()`
   creates an idle source, names it, and attaches it to `task->context`, the
   caller's context, via `g_task_attach_source()`
   (`gio/gtask.c`, `g_task_return()`; on `main` the attach is at ~1461).
3. Separately, `threaded_resolver_worker_cb()` signals the condition variable
   the sync caller is waiting on (`has_returned` + `g_cond_broadcast`). The
   caller wakes, `g_task_propagate_pointer()` succeeds, the caller unrefs its
   task reference and returns. Everything is functionally correct.
4. The completion idle still sits in the caller's context and holds the last
   reference on the `GTask`. If that context is never iterated, nothing ever
   dispatches it: source, closure, name string, `GTask` and its `LookupData`
   stay live. One set per lookup.
5. Independently, `run_task_in_thread_pool_async()` attaches a timeout source
   per lookup to the shared worker context (1526-1534). Those do fire and
   free themselves after the 30 s default timeout, so they are a bounded
   sliding window rather than a leak, but in any 30 s burst they add ~0.4 kB
   per call on top.

### Who is affected

Any program or thread that calls the synchronous resolver API while its
thread-default main context is not being iterated: command-line tools,
daemons without a GLib main loop, and worker threads that pushed a private
context. A conventional GTK/main-loop application drains the idles as they
arrive and only pays the cost transiently, which is presumably why this has
gone unnoticed.

### Suggested direction

The sync paths want completion semantics like `g_task_run_in_thread_sync()`:
the caller is woken by the condition variable, so the completion idle serves
no purpose for them. Marking these tasks as completing synchronously (the
private flag `g_task_run_in_thread_sync()` uses), or otherwise not attaching
`complete_in_idle_cb` to a context the sync wrapper never iterates, would
remove the unbounded half. Whether the per-lookup 30 s timeout source on the
worker context is worth destroying eagerly on completion (2.80.0 already
destroys it in `lookup_data_free()`, but the data lives as long as the task,
which is exactly the problem above) then follows for free.

### Reference program

```c
/* srv-leak.c: reproducer for g_resolver_lookup_service() memory growth.
 *
 * Build:  cc -O2 -o srv-leak srv-leak.c $(pkg-config --cflags --libs gio-2.0)
 *
 * Run:    ./srv-leak srv  <domain> [iterations]   # SRV path (leaks)
 *         ./srv-leak name <domain> [iterations]   # A/AAAA control (flat)
 *
 * The program resolves the SAME domain <iterations> times through the
 * documented, reference-balanced call sequence and prints its own RSS
 * (VmRSS from /proc/self/status) every 500 iterations. A bounded cache
 * would plateau; the SRV path grows linearly instead.
 *
 * "srv" exercises g_resolver_lookup_service() / g_resolver_free_targets().
 * "name" exercises g_resolver_lookup_by_name() / g_resolver_free_addresses()
 * in the identical loop shape, as the control: it stays flat, so the growth
 * is specific to the SRV/records path, not to GResolver usage in general.
 */
#include <gio/gio.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static long
vm_rss_kb (void)
{
  FILE *f = fopen ("/proc/self/status", "r");
  char line[256];
  long kb = -1;

  if (f == NULL)
    return -1;
  while (fgets (line, sizeof line, f) != NULL)
    {
      if (strncmp (line, "VmRSS:", 6) == 0)
        {
          kb = strtol (line + 6, NULL, 10);
          break;
        }
    }
  fclose (f);
  return kb;
}

int
main (int argc, char **argv)
{
  const char *mode = argc > 1 ? argv[1] : "srv";
  const char *domain = argc > 2 ? argv[2] : "example.com";
  int iterations = argc > 3 ? atoi (argv[3]) : 6000;
  int timeout_ms = argc > 4 ? atoi (argv[4]) : -1;  /* -1: leave default */
  int linger_s = argc > 5 ? atoi (argv[5]) : 0;
  gboolean srv = g_str_equal (mode, "srv");
  GResolver *resolver = g_resolver_get_default ();

  if (timeout_ms >= 0)
    g_object_set (resolver, "timeout", (guint) timeout_ms, NULL);
  {
    guint t;
    g_object_get (resolver, "timeout", &t, NULL);
    printf ("GResolver timeout property: %u ms\n", t);
  }
  long rss0 = -1;
  int found = 0, notfound = 0;

  for (int i = 1; i <= iterations; i++)
    {
      GError *error = NULL;

      if (srv)
        {
          GList *targets = g_resolver_lookup_service (
              resolver, "jmap", "tcp", domain, NULL, &error);

          if (targets != NULL)
            g_resolver_free_targets (targets);
        }
      else
        {
          GList *addresses = g_resolver_lookup_by_name (
              resolver, domain, NULL, &error);

          if (addresses != NULL)
            g_resolver_free_addresses (addresses);
        }

      if (error != NULL)
        {
          notfound++;
          g_error_free (error);
        }
      else
        found++;

      if (i % 500 == 0 || i == 1)
        {
          long rss = vm_rss_kb ();
          char delta[80] = "";

          if (rss0 < 0 && i >= 500)
            rss0 = rss; /* baseline after warm-up */
          else if (rss0 > 0 && i > 500)
            snprintf (delta, sizeof delta,
                      "  (+%ld kB, %.2f kB/call since warm-up)",
                      rss - rss0, (rss - rss0) / (double) (i - 500));
          printf ("iter %5d  VmRSS %6ld kB%s\n", i, rss, delta);
          fflush (stdout);
        }
    }

  printf ("done: mode=%s domain=%s found=%d not-found=%d\n",
          mode, domain, found, notfound);
  for (int s = 0; s < linger_s; s += 5)
    {
      g_usleep (5 * G_USEC_PER_SEC);
      printf ("linger %2ds  VmRSS %6ld kB\n", s + 5, vm_rss_kb ());
      fflush (stdout);
    }
  g_object_unref (resolver);
  return 0;
}
```
