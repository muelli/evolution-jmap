/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Headless reproduction client for Roadmap item 22:
 * "A stale client OAuth2Support proxy in the registry turns every token fetch
 * into a consent window."
 *
 * Reproduction sequence:
 * 1. A mock Evolution shell subprocess (Shell 1) connects to the private session
 *    D-Bus bus, acquires a unique name (e.g. :1.X), and exports the
 *    `org.gnome.evolution.dataserver.Source.OAuth2Support` interface.
 * 2. The client queries the OAuth2 token for the source via the proxy to Shell 1,
 *    verifying that it returns a valid token ("mock-token-shell-1").
 * 3. Shell 1 is terminated via SIGTERM (`kill -TERM`). Its unique name (:1.X)
 *    disappears from the D-Bus bus.
 * 4. When a token is requested via the stale proxy pointing to the dead unique
 *    name (:1.X), the call fails immediately with G_DBUS_ERROR_SERVICE_UNKNOWN
 *    ("The name :1.X was not provided by any .service files").
 * 5. A second shell (Shell 2) starts with a new unique name (:1.Y) and exports
 *    the OAuth2Support interface, but because EDS registry does not rebind or
 *    clear the stale proxy on existing sources, calls to the stale proxy continue
 *    to fail with G_DBUS_ERROR_SERVICE_UNKNOWN.
 *
 * usage: functional-oauth2-stale-proxy-client <account-uid>
 */

#include <libedataserver/libedataserver.h>
#include <gio/gio.h>
#include <glib.h>
#include <glib/gstdio.h>
#include <signal.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#define OAUTH2_SUPPORT_OBJECT_PATH "/org/gnome/evolution/dataserver/Source/OAuth2Support"
#define OAUTH2_SUPPORT_INTERFACE "org.gnome.evolution.dataserver.Source.OAuth2Support"

static const gchar introspection_xml[] =
	"<node>"
	"  <interface name='org.gnome.evolution.dataserver.Source.OAuth2Support'>"
	"    <method name='GetAccessToken'>"
	"      <arg type='s' name='access_token' direction='out'/>"
	"      <arg type='i' name='expires_in' direction='out'/>"
	"    </method>"
	"  </interface>"
	"</node>";

static void
handle_method_call (GDBusConnection *connection,
                    const gchar *sender,
                    const gchar *object_path,
                    const gchar *interface_name,
                    const gchar *method_name,
                    GVariant *parameters,
                    GDBusMethodInvocation *invocation,
                    gpointer user_data)
{
	const gchar *token = (const gchar *) user_data;

	if (g_strcmp0 (method_name, "GetAccessToken") == 0) {
		g_dbus_method_invocation_return_value (
			invocation,
			g_variant_new ("(si)", token, 3600));
	}
}

static const GDBusInterfaceVTable interface_vtable = {
	handle_method_call,
	NULL,
	NULL
};

static void
run_mock_shell (const gchar *token_value,
                const gchar *ready_file)
{
	GDBusConnection *connection;
	GDBusNodeInfo *introspection_data;
	GError *error = NULL;
	GMainLoop *loop;
	guint reg_id;

	connection = g_bus_get_sync (G_BUS_TYPE_SESSION, NULL, &error);
	if (!connection) {
		g_printerr ("mock shell: failed to connect to session bus: %s\n", error->message);
		exit (1);
	}

	introspection_data = g_dbus_node_info_new_for_xml (introspection_xml, NULL);
	reg_id = g_dbus_connection_register_object (
		connection,
		OAUTH2_SUPPORT_OBJECT_PATH,
		introspection_data->interfaces[0],
		&interface_vtable,
		g_strdup (token_value),
		g_free,
		&error);

	if (reg_id == 0) {
		g_printerr ("mock shell: failed to register D-Bus object: %s\n", error->message);
		exit (1);
	}

	if (ready_file != NULL) {
		const gchar *unique_name = g_dbus_connection_get_unique_name (connection);
		g_file_set_contents (ready_file, unique_name, -1, NULL);
	}

	loop = g_main_loop_new (NULL, FALSE);
	g_main_loop_run (loop);
}

static pid_t
spawn_shell (const gchar *prog,
             const gchar *token_value,
             const gchar *ready_file,
             gchar **out_unique_name)
{
	GPid pid;
	gchar *argv[] = {
		(gchar *) prog,
		"--mock-shell",
		(gchar *) token_value,
		(gchar *) ready_file,
		NULL
	};
	GError *error = NULL;
	gint i;

	g_remove (ready_file);

	if (!g_spawn_async (NULL, argv, NULL, G_SPAWN_SEARCH_PATH | G_SPAWN_DO_NOT_REAP_CHILD, NULL, NULL, &pid, &error)) {
		g_printerr ("g_spawn_async failed: %s\n", error->message);
		exit (1);
	}

	/* Wait for ready_file to be populated */
	for (i = 0; i < 100; i++) {
		gchar *contents = NULL;
		if (g_file_get_contents (ready_file, &contents, NULL, NULL) && contents && strlen (contents) > 0) {
			*out_unique_name = g_strstrip (contents);
			return pid;
		}
		g_free (contents);
		g_usleep (20000);
	}

	g_printerr ("timed out waiting for mock shell to register on D-Bus\n");
	exit (1);
}

static void
wait_for_name_to_disappear (GDBusConnection *connection,
                            const gchar *unique_name)
{
	gint i;

	for (i = 0; i < 100; i++) {
		GVariant *result;
		GError *error = NULL;

		result = g_dbus_connection_call_sync (
			connection,
			"org.freedesktop.DBus",
			"/org/freedesktop/DBus",
			"org.freedesktop.DBus",
			"GetNameOwner",
			g_variant_new ("(s)", unique_name),
			G_VARIANT_TYPE ("(s)"),
			G_DBUS_CALL_FLAGS_NONE,
			100,
			NULL,
			&error);

		if (result != NULL) {
			g_variant_unref (result);
		} else {
			/* Name owner no longer exists on the bus */
			g_clear_error (&error);
			return;
		}

		g_usleep (20000);
	}
}

gint
main (gint argc,
      gchar **argv)
{
	const gchar *account_uid;
	GDBusConnection *session_bus;
	GDBusProxy *shell_1_proxy;
	GVariant *result;
	GError *error = NULL;
	gchar *shell_1_name = NULL;
	gchar *shell_2_name = NULL;
	gchar *ready_file_1;
	gchar *ready_file_2;
	GPid pid_1, pid_2;
	const gchar *token = NULL;
	gint expires_in = 0;

	if (argc >= 4 && g_strcmp0 (argv[1], "--mock-shell") == 0) {
		run_mock_shell (argv[2], argv[3]);
		return 0;
	}

	if (argc < 2) {
		g_printerr ("usage: %s <account-uid>\n", argv[0]);
		return 1;
	}
	account_uid = argv[1];

	ready_file_1 = g_strdup_printf ("/tmp/mock-shell-1-%d.ready", getpid ());
	ready_file_2 = g_strdup_printf ("/tmp/mock-shell-2-%d.ready", getpid ());

	/* Step 1: Start Shell 1 exporting OAuth2Support on its unique bus name */
	pid_1 = spawn_shell (argv[0], "mock-token-shell-1", ready_file_1, &shell_1_name);
	g_print ("shell-1-pid=%d\n", (gint) pid_1);
	g_print ("shell-1-unique-name=%s\n", shell_1_name);

	session_bus = g_bus_get_sync (G_BUS_TYPE_SESSION, NULL, &error);
	if (!session_bus) {
		g_printerr ("failed to connect to session bus: %s\n", error->message);
		kill (pid_1, SIGKILL);
		waitpid (pid_1, NULL, 0);
		return 1;
	}

	/* Step 2: Create a proxy to Shell 1's OAuth2Support object and verify token fetch succeeds */
	shell_1_proxy = g_dbus_proxy_new_sync (
		session_bus,
		G_DBUS_PROXY_FLAGS_NONE,
		NULL,
		shell_1_name,
		OAUTH2_SUPPORT_OBJECT_PATH,
		OAUTH2_SUPPORT_INTERFACE,
		NULL,
		&error);

	if (!shell_1_proxy) {
		g_printerr ("failed to create proxy to Shell 1: %s\n", error->message);
		kill (pid_1, SIGKILL);
		waitpid (pid_1, NULL, 0);
		return 1;
	}

	result = g_dbus_proxy_call_sync (
		shell_1_proxy,
		"GetAccessToken",
		NULL,
		G_DBUS_CALL_FLAGS_NONE,
		2000,
		NULL,
		&error);

	if (result != NULL) {
		g_variant_get (result, "(&si)", &token, &expires_in);
		g_print ("initial-token-success=1\n");
		g_print ("initial-token=%s\n", token);
		g_print ("initial-expires-in=%d\n", expires_in);
		g_variant_unref (result);
	} else {
		g_print ("initial-token-success=0\n");
		g_print ("initial-token-error=%s\n", error->message);
		g_clear_error (&error);
	}

	/* Step 3: Terminate Shell 1 with SIGTERM */
	kill (pid_1, SIGTERM);
	waitpid (pid_1, NULL, 0);
	g_spawn_close_pid (pid_1);
	wait_for_name_to_disappear (session_bus, shell_1_name);
	g_print ("shell-1-killed=1\n");

	/* Step 4: Request token via the stale proxy pointing to dead unique name (:1.X) */
	result = g_dbus_proxy_call_sync (
		shell_1_proxy,
		"GetAccessToken",
		NULL,
		G_DBUS_CALL_FLAGS_NONE,
		2000,
		NULL,
		&error);

	if (result != NULL) {
		g_print ("token-after-kill-success=1\n");
		g_variant_unref (result);
	} else {
		g_print ("token-after-kill-success=0\n");
		g_print ("token-after-kill-error-domain=%s\n", g_quark_to_string (error->domain));
		g_print ("token-after-kill-error-code=%d\n", error->code);
		g_print ("token-after-kill-error-message=%s\n", error->message);
		g_clear_error (&error);
	}

	/* Step 5: Start Shell 2 (new unique name). Demonstrates that the stale proxy does not auto-rebind. */
	pid_2 = spawn_shell (argv[0], "mock-token-shell-2", ready_file_2, &shell_2_name);
	g_print ("shell-2-pid=%d\n", (gint) pid_2);
	g_print ("shell-2-unique-name=%s\n", shell_2_name);

	/* Call to stale proxy still fails because the binding in the proxy still names the dead peer */
	result = g_dbus_proxy_call_sync (
		shell_1_proxy,
		"GetAccessToken",
		NULL,
		G_DBUS_CALL_FLAGS_NONE,
		2000,
		NULL,
		&error);

	if (result != NULL) {
		g_print ("stale-proxy-still-fails=0\n");
		g_variant_unref (result);
	} else {
		g_print ("stale-proxy-still-fails=1\n");
		g_print ("stale-proxy-error-message=%s\n", error->message);
		g_clear_error (&error);
	}

	/* Clean up */
	kill (pid_2, SIGTERM);
	waitpid (pid_2, NULL, 0);
	g_spawn_close_pid (pid_2);

	g_remove (ready_file_1);
	g_remove (ready_file_2);
	g_free (ready_file_1);
	g_free (ready_file_2);
	g_free (shell_1_name);
	g_free (shell_2_name);
	g_object_unref (shell_1_proxy);
	g_object_unref (session_bus);

	return 0;
}
