/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client half of the collection-backend functional test: an ordinary
 * `ESourceRegistry` consumer, the way the "Add Address Book"/"Add Calendar"
 * machinery in Evolution's account list is one. It knows nothing about JMAP
 * and nothing about the mock server — it connects to the registry, which is
 * what D-Bus-activates `evolution-source-registry` and, through it, loads
 * `module-jmap-backend.so` for the account `.source` file already on disk,
 * then waits for the two children the module's populate/fan-out is expected
 * to write (`docs/manual-test-collection-backend.md`'s whole recipe, done by
 * a real daemon instead of by a person).
 *
 * Everything around it — the scratch XDG tree, the account `.source`
 * keyfile, the private D-Bus session, the mock server and every assertion —
 * belongs to `rust/crates/jmap-functional/tests/collection.rs`, which runs
 * this program and reads its output. So this file has no test framework in
 * it and no notion of what "correct" is: it reports what the registry told
 * it on stdout, one `key=value` line per observation, and exits non-zero the
 * moment a call fails.
 *
 * C rather than Rust for the reason `book-client.c`'s own header gives:
 * `libedataserver`'s client API is a surface no crate in this repository
 * binds.
 *
 *   usage: functional-collection-client <account-uid>
 */

#include <libedataserver/libedataserver.h>

/* How long to wait for the two children to appear before giving up and
 * reporting whatever was found. Generous for the same reason
 * `connection-status.c`'s own wait is: this covers activating the registry
 * and a first populate cycle, and the point of a limit at all is that a
 * populate that never runs fails the run instead of hanging it. */
#define WAIT_SECONDS 30

/* State for one wait: the children of `parent_uid`, refreshed on every tick
 * until there are at least two of them or the deadline passes. */
typedef struct {
	ESourceRegistry *registry;
	const gchar *parent_uid;
	GMainLoop *loop;
	gint64 deadline;
	GList *found; /* owned: the children seen when the loop stopped */
} WaitForChildren;

static GList *
children_of (ESourceRegistry *registry,
              const gchar *parent_uid)
{
	GList *all;
	GList *link;
	GList *children = NULL;

	/* NULL asks for every registered source, regardless of which
	 * extensions it carries — the two children this test waits for are
	 * told apart by their own extension afterwards, not by this query. */
	all = e_source_registry_list_sources (registry, NULL);

	for (link = all; link != NULL; link = g_list_next (link)) {
		ESource *source = E_SOURCE (link->data);

		if (g_strcmp0 (e_source_get_parent (source), parent_uid) == 0)
			children = g_list_prepend (children, g_object_ref (source));
	}

	g_list_free_full (all, g_object_unref);

	return children;
}

static gboolean
wait_for_children_tick_cb (gpointer user_data)
{
	WaitForChildren *wait = user_data;
	GList *children = children_of (wait->registry, wait->parent_uid);

	if (g_list_length (children) >= 2 || g_get_monotonic_time () >= wait->deadline) {
		wait->found = children;
		g_main_loop_quit (wait->loop);

		return G_SOURCE_REMOVE;
	}

	g_list_free_full (children, g_object_unref);

	return G_SOURCE_CONTINUE;
}

/* Polls, rather than connecting to "source-added": the children are written
 * to disk by the backend and picked up by the registry's own file monitor,
 * which is exactly the asynchronous step `docs/manual-test-collection-
 * backend.md` describes as "restart the daemons and look" — there is no
 * single signal this client could wait on that is simpler than asking again
 * a few times a second, the same tradeoff `connection-status.c` already
 * makes for a different property of the same kind of source. */
static GList *
wait_for_children (ESourceRegistry *registry,
                    const gchar *parent_uid)
{
	WaitForChildren wait;
	GSource *tick;

	wait.registry = registry;
	wait.parent_uid = parent_uid;
	wait.loop = g_main_loop_new (NULL, FALSE);
	wait.deadline = g_get_monotonic_time () + WAIT_SECONDS * G_TIME_SPAN_SECOND;
	wait.found = NULL;

	tick = g_timeout_source_new (100);
	g_source_set_callback (tick, wait_for_children_tick_cb, &wait, NULL);
	g_source_attach (tick, NULL);

	g_main_loop_run (wait.loop);

	g_source_destroy (tick);
	g_source_unref (tick);
	g_main_loop_unref (wait.loop);

	return wait.found;
}

/* Reports one child as two lines, `<prefix>-backend-name` and
 * `<prefix>-parent`, so the Rust side can tell an address book from a
 * calendar without guessing at which extension a source carries — and can
 * assert the one property `child_added` is actually responsible for
 * (naming the account as `Parent=`) rather than just counting sources. */
static void
report_child (ESource *source,
              const gchar *extension_name,
              const gchar *prefix)
{
	ESourceBackend *backend;

	if (!e_source_has_extension (source, extension_name))
		return;

	backend = e_source_get_extension (source, extension_name);
	g_print ("%s-backend-name=%s\n", prefix, e_source_backend_get_backend_name (backend));
	g_print ("%s-parent=%s\n", prefix, e_source_get_parent (source));
	g_print ("%s-enabled=%d\n", prefix, e_source_get_enabled (source));
}

gint
main (gint argc,
      gchar **argv)
{
	const gchar *account_uid;
	GError *error = NULL;
	ESourceRegistry *registry;
	ESource *account;
	GList *children;
	GList *link;
	guint address_books = 0;
	guint calendars = 0;

	if (argc != 2) {
		g_printerr ("usage: %s <account-uid>\n", argv[0]);
		return 1;
	}
	account_uid = argv[1];

	/* This is what D-Bus-activates evolution-source-registry, on this
	 * process's private bus, with `EDS_REGISTRY_MODULES` already pointed
	 * at this session's scratch directory by the Rust side — see
	 * `Session::stage_collection_backend`. */
	registry = e_source_registry_new_sync (NULL, &error);
	if (registry == NULL) {
		g_printerr ("e_source_registry_new_sync: %s\n", error->message);
		g_error_free (error);
		return 1;
	}

	account = e_source_registry_ref_source (registry, account_uid);
	if (account == NULL) {
		g_print ("account-found=0\n");
		g_object_unref (registry);
		return 1;
	}
	g_print ("account-found=1\n");

	children = wait_for_children (registry, account_uid);
	g_print ("children-found=%d\n", g_list_length (children));

	for (link = children; link != NULL; link = g_list_next (link)) {
		ESource *child = E_SOURCE (link->data);

		if (e_source_has_extension (child, E_SOURCE_EXTENSION_ADDRESS_BOOK)) {
			report_child (child, E_SOURCE_EXTENSION_ADDRESS_BOOK, "address-book");
			address_books++;
		}
		if (e_source_has_extension (child, E_SOURCE_EXTENSION_CALENDAR)) {
			report_child (child, E_SOURCE_EXTENSION_CALENDAR, "calendar");
			calendars++;
		}
	}
	g_print ("address-books-found=%u\n", address_books);
	g_print ("calendars-found=%u\n", calendars);

	g_list_free_full (children, g_object_unref);
	g_object_unref (account);
	g_object_unref (registry);

	return 0;
}
