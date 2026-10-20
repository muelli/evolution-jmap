/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client half of the shared collection functional test. It is the same
 * `ESourceRegistry` client as `collection-client.c`, but it waits for
 * specific children: the children that a share from a different account
 * adds to the account, or removes when the share stops.
 *
 * The name of a child is `<kind>:<identity>`. `<kind>` is `address-book` or
 * `calendar`. `<identity>` is the `[Resource] Identity`, which is
 * `<accountId>/<collectionId>` for a shared collection. The kind is
 * necessary because a JMAP id is unique only for one type.
 *
 * First, the client reports how many named children the registry loaded
 * from disk, so the Rust side can tell "removed" from "never there". In
 * `present` mode it then waits until all named children exist. In `absent`
 * mode it waits until none exists. A deadline makes a missing populate give
 * a report, not a hang.
 * `rust/crates/jmap-functional/tests/live-stalwart-collection-shared.rs`
 * does the assertions.
 *
 *   usage: functional-collection-shared-client <account-uid> present|absent
 *          <kind>:<identity>...
 */

#include <string.h>

#include <libedataserver/libedataserver.h>

/* Includes the registry start, the authenticate retry and a populate of the
 * account and of all shared accounts. */
#define WAIT_SECONDS 60

typedef struct {
	ESourceRegistry *registry;
	const gchar *parent_uid;
	gchar **identities;
	gboolean want_present;
	GMainLoop *loop;
	gint64 deadline;
} WaitForIdentities;

/* The extension for a `<kind>:` prefix, or NULL. */
static const gchar *
extension_for_kind (const gchar *kind)
{
	if (g_strcmp0 (kind, "address-book") == 0)
		return E_SOURCE_EXTENSION_ADDRESS_BOOK;
	if (g_strcmp0 (kind, "calendar") == 0)
		return E_SOURCE_EXTENSION_CALENDAR;
	return NULL;
}

/* The child of `parent_uid` for `name` (`<kind>:<identity>`), or NULL. The
 * caller owns it. */
static ESource *
child_with_identity (ESourceRegistry *registry,
                     const gchar *parent_uid,
                     const gchar *name)
{
	GList *all;
	GList *link;
	ESource *found = NULL;
	const gchar *colon = strchr (name, ':');
	const gchar *identity;
	gchar *kind;
	const gchar *extension_name;

	if (colon == NULL)
		return NULL;
	kind = g_strndup (name, colon - name);
	extension_name = extension_for_kind (kind);
	g_free (kind);
	if (extension_name == NULL)
		return NULL;
	identity = colon + 1;

	all = e_source_registry_list_sources (registry, extension_name);

	for (link = all; link != NULL && found == NULL; link = g_list_next (link)) {
		ESource *source = E_SOURCE (link->data);
		ESourceResource *resource;

		if (g_strcmp0 (e_source_get_parent (source), parent_uid) != 0 ||
		    !e_source_has_extension (source, E_SOURCE_EXTENSION_RESOURCE))
			continue;

		resource = e_source_get_extension (source, E_SOURCE_EXTENSION_RESOURCE);
		if (g_strcmp0 (e_source_resource_get_identity (resource), identity) == 0)
			found = g_object_ref (source);
	}

	g_list_free_full (all, g_object_unref);

	return found;
}

/* The number of identities that are children now. */
static guint
count_present (ESourceRegistry *registry,
               const gchar *parent_uid,
               gchar **identities)
{
	guint present = 0;
	guint ii;

	for (ii = 0; identities[ii] != NULL; ii++) {
		ESource *child = child_with_identity (registry, parent_uid, identities[ii]);

		if (child != NULL) {
			present++;
			g_object_unref (child);
		}
	}

	return present;
}

static gboolean
wait_tick_cb (gpointer user_data)
{
	WaitForIdentities *wait = user_data;
	guint present = count_present (wait->registry, wait->parent_uid, wait->identities);
	gboolean done;

	if (wait->want_present)
		done = present == g_strv_length (wait->identities);
	else
		done = present == 0;

	if (done || g_get_monotonic_time () >= wait->deadline) {
		g_main_loop_quit (wait->loop);
		return G_SOURCE_REMOVE;
	}

	return G_SOURCE_CONTINUE;
}

/* Polls every 100ms, as `collection-client.c` does. */
static void
wait_for_identities (ESourceRegistry *registry,
                     const gchar *parent_uid,
                     gchar **identities,
                     gboolean want_present)
{
	WaitForIdentities wait;
	GSource *tick;

	wait.registry = registry;
	wait.parent_uid = parent_uid;
	wait.identities = identities;
	wait.want_present = want_present;
	wait.loop = g_main_loop_new (NULL, FALSE);
	wait.deadline = g_get_monotonic_time () + WAIT_SECONDS * G_TIME_SPAN_SECOND;

	tick = g_timeout_source_new (100);
	g_source_set_callback (tick, wait_tick_cb, &wait, NULL);
	g_source_attach (tick, NULL);

	g_main_loop_run (wait.loop);

	g_source_destroy (tick);
	g_source_unref (tick);
	g_main_loop_unref (wait.loop);
}

/* Reports the child for identity `index` as `shared-<index>-*` lines, or
 * only `shared-<index>-found=0`. */
static void
report_identity (ESourceRegistry *registry,
                 const gchar *parent_uid,
                 const gchar *identity,
                 guint index)
{
	ESource *child = child_with_identity (registry, parent_uid, identity);
	const gchar *extension_name = NULL;
	const gchar *kind = NULL;

	if (child == NULL) {
		g_print ("shared-%u-found=0\n", index);
		return;
	}

	if (e_source_has_extension (child, E_SOURCE_EXTENSION_ADDRESS_BOOK)) {
		extension_name = E_SOURCE_EXTENSION_ADDRESS_BOOK;
		kind = "address-book";
	} else if (e_source_has_extension (child, E_SOURCE_EXTENSION_CALENDAR)) {
		extension_name = E_SOURCE_EXTENSION_CALENDAR;
		kind = "calendar";
	}

	g_print ("shared-%u-found=1\n", index);
	g_print ("shared-%u-kind=%s\n", index, kind ? kind : "none");
	g_print ("shared-%u-display-name=%s\n", index, e_source_get_display_name (child));
	g_print ("shared-%u-enabled=%d\n", index, e_source_get_enabled (child));
	g_print ("shared-%u-remote-deletable=%d\n", index, e_source_get_remote_deletable (child));
	if (extension_name != NULL) {
		ESourceBackend *backend = e_source_get_extension (child, extension_name);

		g_print ("shared-%u-backend-name=%s\n", index,
			e_source_backend_get_backend_name (backend));
	}

	g_object_unref (child);
}

gint
main (gint argc,
      gchar **argv)
{
	const gchar *account_uid;
	gboolean want_present;
	gchar **identities;
	GError *error = NULL;
	ESourceRegistry *registry;
	ESource *account;
	const gchar *seed_password;
	guint ii;

	if (argc < 4 || (g_strcmp0 (argv[2], "present") != 0 && g_strcmp0 (argv[2], "absent") != 0)) {
		g_printerr ("usage: %s <account-uid> present|absent <kind>:<identity>...\n", argv[0]);
		return 1;
	}
	account_uid = argv[1];
	want_present = g_strcmp0 (argv[2], "present") == 0;
	identities = &argv[3];

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

	/* What the registry loaded from the `.source` files of the previous run,
	 * before populate can remove a child. */
	g_print ("initially-present=%u\n", count_present (registry, account_uid, identities));

	/* Store the password on the account and authenticate again, as
	 * `collection-client.c` does. */
	seed_password = g_getenv ("JMAP_FUNCTIONAL_STORE_PASSWORD");
	if (seed_password && *seed_password) {
		if (!e_source_store_password_sync (account, seed_password, TRUE, NULL, &error) ||
		    !e_source_invoke_authenticate_sync (account, NULL, NULL, &error)) {
			g_printerr ("authenticate: %s\n", error->message);
			g_error_free (error);
			g_object_unref (account);
			g_object_unref (registry);
			return 1;
		}
	}

	wait_for_identities (registry, account_uid, identities, want_present);

	g_print ("present=%u\n", count_present (registry, account_uid, identities));
	for (ii = 0; identities[ii] != NULL; ii++)
		report_identity (registry, account_uid, identities[ii], ii);

	g_object_unref (account);
	g_object_unref (registry);

	return 0;
}
