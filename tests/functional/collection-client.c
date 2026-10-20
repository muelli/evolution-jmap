/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client half of the collection backend functional test. It is a plain
 * `ESourceRegistry` client and has no JMAP code. Its registry connection
 * starts `evolution-source-registry`, which loads `module-jmap-backend.so`
 * for the account `.source` file. Then it waits for the two children that
 * populate must write. This is the procedure of
 * `docs/manual-test-collection-backend.md`, done by a real daemon.
 *
 * `rust/crates/jmap-functional/tests/collection.rs` (and
 * `live-stalwart-collection.rs` for a real server) creates the scratch XDG
 * tree, the account `.source` keyfile, the private D-Bus session and the
 * server, and it does all the assertions. This program writes one
 * `key=value` line on stdout for each observation. It exits non-zero when a
 * call fails.
 *
 * The file is C because no crate in this repository binds the
 * `libedataserver` client API (see `book-client.c`).
 *
 *   usage: functional-collection-client <account-uid>
 */

#include <libedataserver/libedataserver.h>

/* The maximum wait for the two children. It includes the registry start and
 * the first populate. A limit makes a stuck populate fail the run instead of
 * hang it. */
#define WAIT_SECONDS 30

/* State for one wait for at least two children of `parent_uid`. */
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

	/* NULL gives all sources. The caller sorts the children by extension. */
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

/* Polls every 100ms. The backend writes the children to disk and the file
 * monitor of the registry reads them, so no single signal is simpler. */
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

/* Reports one child as three lines: `<prefix>-backend-name`,
 * `<prefix>-parent` and `<prefix>-enabled`. The Rust side can then check
 * the `Parent=` that `child_added` sets. */
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

	/* Starts evolution-source-registry on the private bus.
	 * `Session::stage_collection_backend` sets `EDS_REGISTRY_MODULES`. */
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

	/* A real server needs a password and no GUI prompt is available. Store
	 * it on the account, because the collection backend authenticates.
	 * Populate already ran without a password when the registry loaded the
	 * account, so it waits on `credentials-required`.
	 * `e_source_invoke_authenticate_sync` with no credentials makes EDS
	 * read the stored password and try again. */
	{
		const gchar *seed_password = g_getenv ("JMAP_FUNCTIONAL_STORE_PASSWORD");

		if (seed_password && *seed_password) {
			if (!e_source_store_password_sync (account, seed_password, TRUE, NULL, &error)) {
				g_printerr ("store-password: %s\n", error->message);
				g_error_free (error);
				g_object_unref (account);
				g_object_unref (registry);
				return 1;
			}
			if (!e_source_invoke_authenticate_sync (account, NULL, NULL, &error)) {
				g_printerr ("invoke-authenticate: %s\n", error->message);
				g_error_free (error);
				g_object_unref (account);
				g_object_unref (registry);
				return 1;
			}
		}
	}

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
