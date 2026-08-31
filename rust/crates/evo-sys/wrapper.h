/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Single translation unit handed to bindgen. Unlike the EDS umbrella headers,
 * Evolution's `mail/` ones install unguarded, so this includes exactly the
 * ones the setup module is written against and lets them pull in what they need
 * (gtk, camel, libebackend and e-util).
 */

#include <mail/e-mail-config-service-backend.h>

/* And the page that class is an extension of, for the one accessor
 * `setup_defaults` needs off it — the address typed on the identity page. The
 * page itself stays an opaque handle (see build.rs); this is here so that the
 * accessor's declaration is. */
#include <mail/e-mail-config-service-page.h>

/* The interface that page implements, for `e_mail_config_page_changed` —
 * `insert_widgets`'s way of telling the assistant an entry changed so
 * `check_complete` is asked again. Not pulled in by the two includes above:
 * `e-mail-config-service-page.h` reaches `e-mail-config-activity-page.h`,
 * which does not include this one either. */
#include <mail/e-mail-config-page.h>

/* The second class this crate's consumers subclass, and the only route by
 * which Evolution's New Address Book and New Calendar dialogs can be made to
 * offer JMAP at all: `ESourceConfig` builds one candidate per registered
 * `ESourceConfigBackend`, and a provider that registers none is absent from the
 * Type list, has a NULL backend when an existing source is edited, and asserts
 * out of `e_source_config_commit` instead of saving.
 *
 * `e-util/`'s headers, unlike `mail/`'s, are guarded: each refuses to be
 * included except from inside the `e-util/e-util.h` umbrella, which announces
 * itself by defining `__E_UTIL_H_INSIDE__` around its 255 includes. Defining
 * that token here and naming the one header we want is the narrow reading of
 * that guard — it says "this is an e-util include", which it is — and it keeps
 * the parse to a single class instead of the whole of Evolution's utility
 * library, WebKit included. The alternative, including the umbrella, would put
 * several hundred classes in front of bindgen for one struct, and would make
 * every allowlist and blocklist below a filter over a surface nobody here has
 * read.
 *
 * It also brings `e-util/e-source-config.h` with it, which is what
 * `e_source_config_backend_get_config` hands back; that class stays an opaque
 * handle (see build.rs), being a GtkBox. */
#define __E_UTIL_H_INSIDE__
#include <e-util/e-source-config-backend.h>
