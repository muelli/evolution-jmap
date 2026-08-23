/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The write half of the collection-backend functional test: an ordinary
 * `ESourceRegistry` consumer driving the exact calls Evolution's own "New
 * Address Book" and "Delete" use — `e_source_remote_create_sync()` on the
 * account with a scratch `ESource` describing the new address book, and
 * `e_source_remote_delete_sync()` on the child that came back. It knows
 * nothing about JMAP: both calls are the same ones any collection backend's
 * children go through, and what makes this a JMAP test at all is that
 * `module-jmap-backend.so` is the one loaded on the private bus this runs
 * on.
 *
 * These are deliberately not `e_source_registry_create_sources_sync()`/
 * `e_source_remove_sync()`, which this file used at first and which turned
 * out to be the wrong pair: those write a source's keyfile to the registry
 * directly with no backend involved at all — the right calls for a
 * standalone account, not a collection's child — and a first run against
 * them silently "succeeded" with a source that named no `BackendName`, made
 * no request to the mock, and refused to be removed. `e_source.h`'s own
 * comments on `remote_create_sync`/`remote_delete_sync` are what named the
 * pair that actually reaches `ECollectionBackendClass::
 * create_resource_sync`/`delete_resource_sync`: the first is called *on the
 * account*, passing the scratch source as an argument, and requires
 * `ESource:remote-creatable`; the second is called *on the child itself* and
 * requires `ESource:remote-deletable`.
 *
 * `collection-client.c` (the sibling this file was split from) proves the
 * *read* direction — populate/fan-out discovering what a server already
 * holds. This proves the *write* direction, which
 * `rust/crates/jmap-backend-collection`'s own tests only ever drive against
 * an in-process `EServerSideSource` it builds itself, never through a real
 * registry's own D-Bus round trip the way this does.
 *
 * Everything around it — the scratch XDG tree, the account `.source`
 * keyfile, the private D-Bus session, the mock server and every assertion —
 * belongs to `rust/crates/jmap-functional/tests/collection-create.rs`, which
 * runs this program and reads its output. So this file has no test framework
 * in it and no notion of what "correct" is: it reports what the registry
 * told it on stdout, one `key=value` line per observation, and exits
 * non-zero the moment a call fails.
 *
 *   usage: functional-collection-create-client <account-uid>
 */

#include <libedataserver/libedataserver.h>

/* How long to wait for an asynchronous property to settle before giving up.
 * Generous for the reason `collection-client.c`'s own wait is: this covers
 * activating the registry, a first populate cycle, and the registry's own
 * file-monitor pickup of a just-written or just-removed child, and the point
 * of a limit at all is that a wedged backend fails the run instead of
 * hanging it. */
#define WAIT_SECONDS 30

/* Polls `condition` every 100ms until it returns TRUE or `WAIT_SECONDS`
 * elapses, running a nested main loop between checks — the same shape
 * `collection-client.c::wait_for_children` uses for a different property of
 * the same kind of source, since none of what this waits for (a
 * `remote-creatable` flag flipped by a populate, a child appearing or
 * disappearing) has a simpler signal to wait on than asking again a few
 * times a second. */
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

/* The one address-book child of `parent_uid`, or NULL — this account has
 * none until the create below makes one, so "found one" and "found the one
 * this test made" are the same question. */
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
	ESource *found; /* borrowed: set once found, read after the wait */
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

/* Reports the created child's properties the same way `collection-
 * client.c::report_child` does, so the Rust side reads the same shape of
 * `key=value` lines for both the discovered and the created case. */
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

	/* `e_source_remote_create_sync` requires this — it is only set once
	 * the first populate has decided the account has somewhere to create
	 * a collection in, so the create below is unreachable until this is
	 * TRUE. */
	if (!wait_until (account_is_remote_creatable, account)) {
		g_print ("account-creatable=0\n");
		g_object_unref (account);
		g_object_unref (registry);
		return 1;
	}
	g_print ("account-creatable=1\n");

	/* The scratch source "New Address Book" hands the account: no
	 * `GDBusObject` yet, the `[Address Book]` extension naming what kind
	 * of collection this is (forced into existence by asking for it, the
	 * same way a real account-editor dialog's widgets do), and a display
	 * name (the mock's own `AddressBook/set` handler refuses an empty
	 * one). No uid and no `Parent=` — `e_source_remote_create_sync`
	 * already knows the parent from `account`, and
	 * `create_resource.rs::adopt_created` sets `Parent=` itself on
	 * whatever uid the registry service ends up minting for it. */
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

	/* The registry service mints its own uid for the child rather than
	 * keeping the scratch source's (`e_server_side_source_new_user_file`,
	 * per `create_resource.rs`'s own module comment), so it is found the
	 * way `collection-client.c` finds discovered children: by asking
	 * which address book names this account as `Parent=`. */
	find.registry = registry;
	find.parent_uid = account_uid;
	find.found = NULL;
	if (!wait_until (child_appeared, &find)) {
		g_print ("created=0\n");
		g_object_unref (registry);
		return 1;
	}
	created = find.found;
	find.found = NULL; /* ownership moved to `created`; avoid a dangling alias */
	report_created (created);

	/* "Delete": `e_source_remote_delete_sync` on the very source the
	 * create handed back, which is what
	 * `server_side_source_remote_delete_sync` needs the child's own
	 * `remote-deletable` flag for — `jmap-backend-collection::
	 * child_added` sets it on every child on publish, this one included.
	 * Unlike `e_source_remove_sync` (which this file used at first, and
	 * which fails outright on a backend-owned child that is not
	 * `removable` — see the file header), this is the destructive,
	 * server-reaching delete the same way `remote_create_sync` is the
	 * server-reaching create. */
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
