<!--
SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
SPDX-License-Identifier: GPL-3.0-or-later
-->

# DRAFT, not filed

Target: `GNOME/evolution-data-server`, issue. Written 2026-08-31 against EDS
3.52.3, found 2026-08-24 (session N+60) while root-causing an unrelated
crash that this line of log noise needed ruling out first. It is not related
to that crash and is not our bug. Filed only after a maintainer has read it,
same rule as the other drafts in this directory.

---

## `CamelMimePart` notifies a `content-location` property it never installs

### What happens

Setting a MIME part's Content-Location header prints, on every call:

```
GLib-GObject-CRITICAL **: g_object_notify: object class 'CamelMimePart' has no property named 'content-location'
```

Our own `eds-sys/tests/camel.rs` MIME-part probe triggers this on every run
by calling `camel_mime_part_set_content_location()`, which is why we noticed
it, but any caller of that function hits the same warning.

### Root cause, read from `evolution-data-server` 3.52.3's own source

`src/camel/camel-mime-part.c`:

- `PROP_CONTENT_LOCATION` is declared in the property enum, right between
  `PROP_CONTENT_ID` and `PROP_CONTENT_MD5` (line 74).
- `mime_part_set_property()` handles the `PROP_CONTENT_LOCATION` case (line
  382), forwarding to `camel_mime_part_set_content_location()`.
- `mime_part_get_property()` handles it too (line 423), forwarding to
  `camel_mime_part_get_content_location()`.
- `camel_mime_part_set_content_location()` ends with
  `g_object_notify (G_OBJECT (mime_part), "content-location")` (line 1379).
- But `camel_mime_part_class_init()` (starting line 1137) calls
  `g_object_class_install_property()` for `PROP_CONTENT_ID` (line 1166),
  `PROP_CONTENT_MD5` (line 1178), `PROP_DESCRIPTION` (line 1190) and
  `PROP_DISPOSITION` (line 1202) — installing every neighbour in the enum
  **except** `PROP_CONTENT_LOCATION`, which is skipped entirely between the
  `PROP_CONTENT_ID` and `PROP_CONTENT_MD5` blocks.

So the property is a real, fully-wired GObject property everywhere except
the one place (`class_init`) that has to register it with the type system.
`g_object_notify()` looks the property up by name on the class each time it
is called, finds nothing, and logs the `CRITICAL`. `get`/`set_property()`
work fine standalone because they are plain C switch cases reached directly
by `camel_mime_part_set_content_location()`/`_get_content_location()`, not
through GObject's property machinery — the header itself is written and
read correctly. Only the notification, and anything that would go through
`g_object_get`/`set_property("content-location", …)` or property
bindings/introspection, is affected.

### Consequence

Log noise only, verified against our own use: the setter's own effect (the
`Content-Location` header ending up on the part) is unaffected, since it is
set via `camel_medium_set_header()` before the broken notify call ever
runs. Nothing downstream that reads the header back is affected either. The
only visible symptom is the `GLib-GObject-CRITICAL` line, plus the property
being unreachable via `g_object_get_property (part, "content-location", …)`
or a `GBinding`.

### Suggested change

Add the missing installation in `camel_mime_part_class_init()`, matching its
neighbours exactly (`content-id` and `content-md5` are the closest models,
both plain nullable strings with the same flags):

```c
	g_object_class_install_property (
		object_class,
		PROP_CONTENT_LOCATION,
		g_param_spec_string (
			"content-location",
			"Content Location",
			NULL,
			NULL,
			G_PARAM_READWRITE |
			G_PARAM_EXPLICIT_NOTIFY |
			G_PARAM_STATIC_STRINGS));
```

Placed between the existing `PROP_CONTENT_ID` and `PROP_CONTENT_MD5`
`g_object_class_install_property()` blocks (i.e. before line 1178 in the
3.52.3 source) keeps the property declarations in the same order as the
enum. One line's worth of a real fix; the rest of the diff is the
boilerplate `g_param_spec_string()` call each sibling property already has.
