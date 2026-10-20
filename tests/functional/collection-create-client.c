/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The write half of the collection backend functional test. It is a plain
 * `ESourceRegistry` client that makes the calls of the Evolution "New
 * Address Book" and "Delete" actions: `e_source_remote_create_sync()` on the
 * account with a scratch `ESource`, then `e_source_remote_delete_sync()` on
 * the new child. The file has no JMAP code. The JMAP part is that
 * `module-jmap-backend.so` is the loaded collection backend.
 *
 * Do not use `e_source_registry_create_sources_sync()` or
 * `e_source_remove_sync()` here. They write the keyfile directly and do not
 * call the backend. Only the remote pair reaches
 * `ECollectionBackendClass::create_resource_sync`/`delete_resource_sync`.
 * The create call goes to the account and needs `ESource:remote-creatable`.
 * The delete call goes to the child and needs `ESource:remote-deletable`.
 *
 * `collection-client.c` tests the read direction. This file tests the write
 * direction through a real registry over D-Bus. The unit tests in
 * `rust/crates/jmap-backend-collection` use only an in-process
 * `EServerSideSource`.
 *
 * `rust/crates/jmap-functional/tests/collection-create.rs` creates the
 * scratch XDG tree, the account `.source` keyfile, the private D-Bus session
 * and the mock server, and it does all the assertions. This program writes
 * one `key=value` line on stdout for each observation. It exits non-zero
 * when a call fails.
 *
 *   usage: functional-collection-create-client <account-uid>
 */

#include <libedataserver/libedataserver.h>

/* The maximum wait for an asynchronous property. It includes the registry
 * start, the first populate and the file monitor. A limit makes a stuck
 * backend fail the run instead of hang it. */
#define WAIT_SECONDS 30

/* Polls `condition` every 100ms in a nested main loop, until it is TRUE or
 * `WAIT_SECONDS` is over. `collection-client.c::wait_for_children` uses the
 * same method. */
typedef gboolean (*PollCondition) (gpointer user_data);

typedef struct {
	PollCondition condition;
	gpointer user_data;
	GMainLoop *loop;
	gint64 deadline;
	gboolean satisfied;
} Poll;

static gboolean
poll_tick_cb (gpointer user_data)
{
	Poll *poll = user_data;

	if (poll->condition (poll->user_data) || g_get_monotonic_time () >= poll->deadline) {
		poll->satisfied = poll->condition (poll->user_data);
		g_main_loop_quit (poll->loop);
		return G_SOURCE_REMOVE;
	}

	return G_SOURCE_CONTINUE;
}

static gboolean
wait_until (PollCondition condition,
            gpointer user_data)
{
	Poll poll;
	GSource *tick;

	if (condition (user_data))
		return TRUE;

	poll.condition = condition;
	poll.user_data = user_data;
	poll.loop = g_main_loop_new (NULL, FALSE);
	poll.deadline = g_get_monotonic_time () + WAIT_SECONDS * G_TIME_SPAN_SECOND;
	poll.satisfied = FALSE;

	tick = g_timeout_source_new (100);
	g_source_set_callback (tick, poll_tick_cb, &poll, NULL);
	g_source_attach (tick, NULL);

	g_main_loop_run (poll.loop);

	g_source_destroy (tick);
	g_source_unref (tick);
	g_main_loop_unref (poll.loop);

	return poll.satisfied;
}

static gboolean
account_is_remote_creatable (gpointer user_data)
{
	ESource *account = user_data;

	return e_source_get_remote_creatable (account);
}

/* The address book child of `parent_uid`, or NULL. The account has no
 * other child, so the child found is the one that this test creates. */
static ESource *
address_book_child_of (ESourceRegistry *registry,
                        const gchar *parent_uid)
{
	GList *all = e_source_registry_list_sources (registry, E_SOURCE_EXTENSION_ADDRESS_BOOK);
	GList *link;
	ESource *found = NULL;

	for (link = all; link != NULL && found == NULL; link = g_list_next (link)) {
		ESource *candidate = E_SOURCE (link->data);

		if (g_strcmp0 (e_source_get_parent (candidate), parent_uid) == 0)
			found = g_object_ref (candidate);
	}

	g_list_free_full (all, g_object_unref);

	return found;
}

typedef struct {
	ESourceRegistry *registry;
	const gchar *parent_uid;
	ESource *found; /* owned: set when found, read after the wait */
} FindChild;

static gboolean
child_appeared (gpointer user_data)
{
	FindChild *find = user_data;

	g_clear_object (&find->found);
	find->found = address_book_child_of (find->registry, find->parent_uid);

	return find->found != NULL;
}

static gboolean
child_is_gone (gpointer user_data)
{
	FindChild *find = user_data;
	ESource *still_there = address_book_child_of (find->registry, find->parent_uid);
	gboolean gone = still_there == NULL;

	g_clear_object (&still_there);

	return gone;
}

/* Reports the created child in the same `key=value` format as
 * `collection-client.c::report_child`. */
static void
report_created (ESource *source)
{
	ESourceBackend *backend;

	g_print ("created=1\n");

	if (!e_source_has_extension (source, E_SOURCE_EXTENSION_ADDRESS_BOOK)) {
		g_print ("created-backend-name=\n");
		return;
	}

	backend = e_source_get_extension (source, E_SOURCE_EXTENSION_ADDRESS_BOOK);
	g_print ("created-backend-name=%s\n", e_source_backend_get_backend_name (backend));
	g_print ("created-parent=%s\n", e_source_get_parent (source));
	g_print ("created-enabled=%d\n", e_source_get_enabled (source));
	g_print ("created-writable=%d\n", e_source_get_writable (source));
}

gint
main (gint argc,
      gchar **argv)
{
	const gchar *account_uid;
	GError *error = NULL;
	ESourceRegistry *registry;
	ESource *account;
	ESource *scratch;
	ESource *created;
	FindChild find;
	gboolean removed;

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

	/* `e_source_remote_create_sync` needs this flag. The first populate
	 * sets it. */
	if (!wait_until (account_is_remote_creatable, account)) {
		g_print ("account-creatable=0\n");
		g_object_unref (account);
		g_object_unref (registry);
		return 1;
	}
	g_print ("account-creatable=1\n");

	/* The scratch source, as "New Address Book" makes it: an
	 * `[Address Book]` extension and a display name (the mock refuses an
	 * empty name). It has no uid and no `Parent=`:
	 * `create_resource.rs::adopt_created` sets `Parent=`. */
	scratch = e_source_new (NULL, NULL, &error);
	if (scratch == NULL) {
		g_printerr ("e_source_new: %s\n", error->message);
		g_error_free (error);
		g_object_unref (account);
		g_object_unref (registry);
		return 1;
	}
	e_source_get_extension (scratch, E_SOURCE_EXTENSION_ADDRESS_BOOK);
	e_source_set_display_name (scratch, "Functional Created Address Book");

	if (!e_source_remote_create_sync (account, scratch, NULL, &error)) {
		g_printerr ("e_source_remote_create_sync: %s\n", error->message);
		g_error_free (error);
		g_object_unref (scratch);
		g_object_unref (account);
		g_object_unref (registry);
		return 1;
	}
	g_object_unref (scratch);
	g_object_unref (account);

	/* The registry gives the child a new uid
	 * (`e_server_side_source_new_user_file`). Thus, find the child by its
	 * `Parent=`, as `collection-client.c` does. */
	find.registry = registry;
	find.parent_uid = account_uid;
	find.found = NULL;
	if (!wait_until (child_appeared, &find)) {
		g_print ("created=0\n");
		g_object_unref (registry);
		return 1;
	}
	created = find.found;
	find.found = NULL; /* `created` owns it now */
	report_created (created);

	/* "Delete" deletes the child on the server. It needs the
	 * `remote-deletable` flag, which `child_added` in
	 * `jmap-backend-collection` sets on each child. */
	removed = e_source_remote_delete_sync (created, NULL, &error);
	g_object_unref (created);
	if (!removed) {
		g_printerr ("e_source_remote_delete_sync: %s\n", error->message);
		g_error_free (error);
		g_object_unref (registry);
		return 1;
	}

	if (!wait_until (child_is_gone, &find)) {
		g_print ("deleted=0\n");
		g_object_unref (registry);
		return 1;
	}
	g_print ("deleted=1\n");

	g_object_unref (registry);

	return 0;
}
