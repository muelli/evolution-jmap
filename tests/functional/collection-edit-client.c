/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Item 63's open question: `jmap-backend-collection`'s `source_changed`
 * module connects the account source's own "changed" signal so that editing
 * a broken account's host gets a fresh populate/authenticate instead of
 * sitting dead until something else reconnects it
 * (`rust/crates/jmap-backend-collection/src/source_changed.rs`). Nothing
 * below `jmap_functional` can drive a real edit of a *live* account against a
 * real registry, so the live-Evolution caveat stayed open after item 63
 * shipped.
 *
 * This client is that edit, made through the same registry API the Account
 * Editor's own "OK" button uses: load the account `ESource`, change a
 * setting, call `e_source_write_sync`. EDS does not care who called it: the
 * registry re-emits "changed" on its own in-process copy of the source
 * either way, which is what `source_changed`'s handler listens for. So this
 * is a faithful, deterministic stand-in for the AT-SPI session
 * `ci/gui-smoke-test.md`'s harness would otherwise need, for a question that
 * is about the registry and the backend, not about the account editor's
 * widget tree.
 *
 * The account starts pointed at a port nothing listens on, so its first
 * populate's fan-out fails before this client even connects (anonymous
 * auth, no credentials round trip to wait on). This client then edits the
 * account's port to the one the caller names (the mock's real port) and
 * writes it back. What happened is read off the mock's own request count by
 * the Rust side after this process exits, not reported here: this program
 * has no notion of what "correct" is, like every other client in this
 * directory.
 *
 *   usage: functional-collection-edit-client <account-uid> <new-port>
 */

#include <stdlib.h>
#include <libedataserver/libedataserver.h>

/* How long to let one phase settle before moving on or exiting. Both waits
 * are generous for the same reason `collection-client.c`'s own
 * `WAIT_SECONDS` is, but need no polling condition: a loopback connection
 * refusal is immediate, and there is nothing locally observable to poll for
 * between "wrote the edit" and "the backend finished retrying" short of the
 * mock's own request count, which belongs to the Rust side. */
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

	/* D-Bus-activates evolution-source-registry on this process's private
	 * bus, which is what runs the account's first populate, against the
	 * closed port the keyfile names, before this client has done anything
	 * else. */
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

	/* Let the closed-port attempt fail. Loopback connection refusal is
	 * immediate, so this is slack, not a requirement. */
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

	/* The same commit the account editor's "OK" makes: the registry writes
	 * the keyfile and re-emits "changed" on its own in-process ESource,
	 * which is what `connect_account_changed` listens on. */
	if (!e_source_write_sync (account, NULL, &error)) {
		g_printerr ("e_source_write_sync: %s\n", error->message);
		g_error_free (error);
		g_print ("write-ok=0\n");
		g_object_unref (account);
		g_object_unref (registry);
		return 1;
	}
	g_print ("write-ok=1\n");

	/* Let a repopulate this edit triggers, if any, reach the mock before
	 * this process exits and tears the private bus (and the registry with
	 * it) down. */
	settle ();

	g_object_unref (account);
	g_object_unref (registry);

	return 0;
}
