/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * `jmap-backend-collection` connects the "changed" signal of the account
 * source (`connect_account_changed`, see
 * `rust/crates/jmap-backend-collection/src/source_changed.rs`). Thus an edit
 * of the host of a broken account starts a new populate and authenticate.
 * This client tests that with a live account and a real registry.
 *
 * The client makes the edit with the registry API that the "OK" button of
 * the Account Editor uses: it changes a setting and calls
 * `e_source_write_sync`. The registry then emits "changed" on its own copy
 * of the source. This replaces the AT-SPI session of
 * `docs/gui-smoke-test.md`, because the test is about the registry and the
 * backend, not about the widgets.
 *
 * At the start, the account points to a closed port, so the first fan-out
 * fails. The client then changes the port to the port of the mock and
 * writes the account. The Rust side reads the request count of the mock
 * after this process exits. This program does not decide what is correct.
 *
 *   usage: functional-collection-edit-client <account-uid> <new-port>
 */

#include <stdlib.h>
#include <libedataserver/libedataserver.h>

/* The time to let one phase complete. There is no condition to poll: only
 * the mock request count shows the result, and the Rust side reads it. */
#define SETTLE_SECONDS 3

static void
settle (void)
{
	GMainLoop *loop = g_main_loop_new (NULL, FALSE);

	g_timeout_add_seconds (SETTLE_SECONDS, (GSourceFunc) g_main_loop_quit, loop);
	g_main_loop_run (loop);
	g_main_loop_unref (loop);
}

gint
main (gint argc,
      gchar **argv)
{
	const gchar *account_uid;
	guint16 new_port;
	GError *error = NULL;
	ESourceRegistry *registry;
	ESource *account;
	ESourceAuthentication *auth;

	if (argc != 3) {
		g_printerr ("usage: %s <account-uid> <new-port>\n", argv[0]);
		return 1;
	}
	account_uid = argv[1];
	new_port = (guint16) atoi (argv[2]);

	/* Starts evolution-source-registry, which runs the first populate
	 * against the closed port. */
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

	/* Let the attempt on the closed port fail. A loopback refusal is
	 * immediate, so this wait is only a margin. */
	settle ();

	if (!e_source_has_extension (account, E_SOURCE_EXTENSION_AUTHENTICATION)) {
		g_print ("has-authentication-extension=0\n");
		g_object_unref (account);
		g_object_unref (registry);
		return 1;
	}
	g_print ("has-authentication-extension=1\n");

	auth = E_SOURCE_AUTHENTICATION (e_source_get_extension (account, E_SOURCE_EXTENSION_AUTHENTICATION));
	e_source_authentication_set_port (auth, new_port);

	/* The same write as "OK" in the account editor. The registry writes the
	 * keyfile and emits "changed", which `connect_account_changed` gets. */
	if (!e_source_write_sync (account, NULL, &error)) {
		g_printerr ("e_source_write_sync: %s\n", error->message);
		g_error_free (error);
		g_print ("write-ok=0\n");
		g_object_unref (account);
		g_object_unref (registry);
		return 1;
	}
	g_print ("write-ok=1\n");

	/* Let a new populate reach the mock before the exit stops the private
	 * bus and the registry. */
	settle ();

	g_object_unref (account);
	g_object_unref (registry);

	return 0;
}
