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
 * which does not include this one either.
 *
 * Since the vacation page, the *interface struct* is generated too (see
 * ALLOWED_TYPES): jmap-ui registers its own EMailConfigPage implementer and
 * fills the vtable's slots, so its layout matters the way a subclassed
 * class struct's does. The page instance stays an opaque handle. */
#include <mail/e-mail-config-page.h>

/* The account editor's page container, which jmap-ui's vacation extension
 * extends (`EExtensionClass::extensible_type = E_TYPE_MAIL_CONFIG_NOTEBOOK`)
 * and adds its page to. Only the editor builds one — the new-account
 * assistant composes pages directly — which is exactly the wanted gate: a
 * vacation responder can only be read from a server an account already
 * names. The class stays an opaque handle (a GtkNotebook subclass); what is
 * generated is its GType accessor, `add_page`, and the two source getters
 * the extension gates and connects with. */
#include <mail/e-mail-config-notebook.h>

/* The composer, for jmap-ui's scheduled send: the extensible its extension
 * names, the editor whose GtkUIManager the "Send Later" actions merge into,
 * the header table whose identity-uid property re-gates them on every From
 * switch, and the async pair that builds the final CamelMimeMessage. All of
 * them opaque handles (GtkWindow/GObject subclasses); see build.rs. */
#include <composer/e-msg-composer.h>

/* The mail window and the detached message window, for jmap-ui's snooze: the
 * two extensibles (EShellView, gated to the view named "mail", and
 * EMailBrowser), the GtkUIManager each merges the popup item into, and the
 * EMailReader face both hand the action its selection and folder through.
 * e-shell-view.h reaches e-shell-window.h and e-shell.h itself; everything
 * stays an opaque handle. */
#include <shell/e-shell-view.h>
#include <mail/e-mail-reader.h>
#include <mail/e-mail-browser.h>

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

/* The two `ESourceConfig` subclasses a backend declares itself an extension
 * *of*, which is the whole of how a candidate reaches one dialog and not the
 * other. `EExtensionClass::extensible_type` is a single GType, so a subclass
 * registered against `ESourceConfig` itself would appear in New Address Book
 * and New Calendar alike and hand each the other's scratch source; Evolution's
 * own modules name the derived class instead (`module-book-config-local` takes
 * `E_TYPE_BOOK_SOURCE_CONFIG`, `module-cal-config-caldav`
 * `E_TYPE_CAL_SOURCE_CONFIG`), and so do ours.
 *
 * Only their `get_type` accessors are generated, plus the one call that makes
 * the calendar side correct: `e_cal_source_config_get_source_type`. The same
 * `ECalSourceConfig` serves New Calendar, New Task List and New Memo List, and
 * distinguishes them by that value alone — so a calendar backend that does not
 * consult it offers JMAP in all three, and a JMAP account has no task or memo
 * collections to create. Both classes stay opaque handles (see build.rs), being
 * `ESourceConfig`s and so `GtkBox`es. */
#include <e-util/e-book-source-config.h>
#include <e-util/e-cal-source-config.h>

/* Evolution's own date entry, for the vacation page's from/to fields: a text
 * entry with a calendar popup, and — the reason it is worth binding rather
 * than hand-rolling — an explicit "no date set" state, which is exactly what
 * RFC 8621 §8's nullable `fromDate`/`toDate` mean. Freeing the page from
 * parsing typed dates at all. It is a GtkHBox subclass, so the class struct
 * stays out and the type joins EVO_HANDLES (see build.rs). */
#include <e-util/e-dateedit.h>
