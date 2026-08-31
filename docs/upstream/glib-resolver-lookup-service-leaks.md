<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# DRAFT, not filed

Target: `GNOME/glib`, issue. Written 2026-08-31 against GLib/GIO 2.80.0,
found 2026-08-19 while building `jmap-backend-core/src/resolver.rs`
(`SystemResolver::lookup_srv`, our `_jmap._tcp` SRV bootstrap lookup). Filed
only after a maintainer has read it, same rule as the other drafts in this
directory.

---

## `g_resolver_lookup_service()` leaks about 1 kB per call

### What happens

Repeatedly resolving the same SRV record through the documented, reference-
balanced sequence leaks memory linearly, on both the found and the not-found
path:

```c
GResolver *resolver = g_resolver_get_default ();
GError *error = NULL;
GList *targets = g_resolver_lookup_service (
        resolver, "jmap", "tcp", "example.com", NULL, &error);

if (targets != NULL)
        g_resolver_free_targets (targets);
if (error != NULL)
        g_error_free (error);

g_object_unref (resolver);
```

Every reference this code acquires is released: `g_resolver_get_default()`'s
strong ref is matched by `g_object_unref()`, a non-NULL `targets` list is
freed with `g_resolver_free_targets()`, and a set `error` is freed with
`g_error_free()`. There is no missing unref on our side, and running the loop
6000 times against the same domain shows RSS growing at a steady ~1 kB per
call rather than plateauing.

### Why this is a GLib bug, not ours

A minimal C reference program doing exactly the sequence above, called in a
tight loop, reproduces the same ~1 kB/call growth. For contrast, the same
loop shape built around `g_resolver_lookup_by_name()` /
`g_resolver_free_addresses()` instead is flat (delta 0 kB after warm-up), so
whatever is leaking is specific to the SRV/records code path inside
`GResolver`, not to `GResolver` usage in general. It is also not a bounded
per-domain cache that would plateau: looking up the *same* domain 6000 times
keeps growing linearly with no sign of leveling off, on both the
record-found and record-not-found branches.

### Reference program

```c
/* srv-leak.c — repro for g_resolver_lookup_service() growth.
 * Build:  cc -o srv-leak srv-leak.c $(pkg-config --cflags --libs gio-2.0)
 * Run:    ./srv-leak example.com 6000   # then watch RSS, e.g. via /proc/<pid>/status
 */
#include <gio/gio.h>
#include <stdlib.h>

int
main (int argc, char **argv)
{
        const gchar *domain = argc > 1 ? argv[1] : "example.com";
        gint iterations = argc > 2 ? atoi (argv[2]) : 6000;
        GResolver *resolver = g_resolver_get_default ();

        for (gint i = 0; i < iterations; i++) {
                GError *error = NULL;
                GList *targets = g_resolver_lookup_service (
                        resolver, "jmap", "tcp", domain, NULL, &error);

                if (targets != NULL)
                        g_resolver_free_targets (targets);
                if (error != NULL)
                        g_error_free (error);
        }

        g_object_unref (resolver);
        return 0;
}
```

Pass a domain with no `_jmap._tcp` record (or an unreachable resolver, e.g. a
domain under a firewalled test network) to exercise the not-found path; both
paths leak at the same rate in our measurements.

### Frequency in practice (context, not an excuse)

This does not block us today: `lookup_srv` runs once per
`ConnectTarget::Domain` connect attempt — a backend `connect_sync`, a
collection account's fan-out authentication, or a user clicking "Look Up
Account Details" — never once per sync poll or per JMAP method call. A
long-running `evolution-source-registry` process therefore loses on the
order of tens to hundreds of kB over its lifetime, and only for accounts set
up from a bare email domain (an explicit `host:port` endpoint is a
`ConnectTarget::Origin` and is never resolved via SRV). It is reported
because it is real and reproducible outside our code, not because it is
urgent.

### Suggested change

Not diagnosed further than "the SRV/records path leaks and the plain
name-lookup path does not" — that distinction is offered as the starting
point for whoever picks this up, since it should narrow the search inside
`gresolver.c`/`gthreadedresolver.c` to the code specific to
`g_resolver_lookup_service()` / `g_resolver_lookup_records()` (they likely
share implementation) rather than the address-lookup machinery both paths
have in common.
