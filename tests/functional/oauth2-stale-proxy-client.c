/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client half of a headless reproduction. A stale
 * `Source.OAuth2Support` interface proxy changes a silent token fetch into
 * `G_DBUS_ERROR_SERVICE_UNKNOWN`. A live capture recorded this failure ("The
 * name :1.4 was not provided by any .service files"), and then a consent
 * window opened.
 *
 * This program is a usual `libedataserver` consumer in place of a calendar
 * or address-book *factory* process. It keeps an `ESource` across a registry
 * restart, as a long-running factory does, and then asks for an OAuth 2.0
 * access token. It knows only the account UID and does not assert.
 * `rust/crates/jmap-functional/tests/oauth2-stale-proxy.rs` runs it and
 * examines the `key=value` lines on stdout.
 *
 * Why this is the real failure, not only D-Bus behaviour
 * (EDS 3.52.3 / Evolution 3.52.4 source):
 *
 *   1. The *registry* exports the interface, not the Evolution shell. In
 *      evolution-3.52.4, `grep -rn OAuth2Support` finds one file, a
 *      mail-config summary widget. The doc of
 *      `e_server_side_source_set_oauth2_support` says: "If @oauth2_support
 *      is non-NULL, the OAuth2Support D-Bus interface is exported at the
 *      object path for @source."
 *   2. The EDS registry module `module-oauth2-services.c:139` calls that
 *      setter. Its `EOAuth2SourceMonitor` does this for each server-side
 *      source whose `[Authentication] Method` names a registered
 *      `EOAuth2Service`. For JMAP, this is `jmap_config::oauth2_service::NAME`
 *      ("JMAP"). Put that module next to `module-jmap-backend.so`:
 *      `EDS_REGISTRY_MODULES` *replaces* the EDS module directory
 *      (`e-source-registry-server.c:1073`).
 *   3. On the client, `e-source.c::source_get_oauth2_access_token_sync` uses
 *      the in-process `e_oauth2_services_find` fallback only when the D-Bus
 *      interface is ABSENT. Otherwise the call goes over the bus through a
 *      `GDBusObjectManagerClient` proxy to the *unique* name of the manager
 *      (`gdbusobjectmanagerclient.c` sets "g-name" to `name_owner` and
 *      asserts `g_dbus_is_unique_name`).
 *   4. The stale proxy is deterministic, not a race. When the registry
 *      stops, `source_registry_object_removed_cb` sees no name owner and
 *      calls `source_registry_object_removed_no_owner`. That function only
 *      removes the object path. It does NOT call
 *      `__e_source_private_replace_dbus_object (source, NULL)`: only the
 *      *by_owner* branch does, when a source is deleted while the service
 *      runs. Thus an `ESource` in a factory keeps its dead proxy, and
 *      main-loop iterations do not remove it while the name has no owner.
 *
 * This program observes steps 1, 3 and 4 directly. Step 1 is necessary: all
 * other observations depend on `oauth2-support-exported`.
 *
 *   usage: functional-oauth2-stale-proxy-client <account-uid>
 */

#include <libedataserver/libedataserver.h>
#include <signal.h>
#include <string.h>

/* The interface that `e_server_side_source_set_oauth2_support` exports. The
 * generated `EDBusSourceOAuth2Support` accessors are in the private header
 * `src/private/e-dbus-source.h`, which EDS does not install. Thus use the
 * name with `g_dbus_object_get_interface` on the public
 * `e_source_ref_dbus_object`. From `src/private/
 * org.gnome.evolution.dataserver.Source.xml:210`. */
#define OAUTH2_SUPPORT_INTERFACE "org.gnome.evolution.dataserver.Source.OAuth2Support"

/* The maximum wait for the unique name of the killed registry to leave the
 * bus. The bus learns of the disconnection asynchronously. If the registry
 * does not stop, the limit makes the run fail and not hang. */
#define NAME_GONE_TIMEOUT_SECONDS 10

/* Write one `key=value` line to stdout for `jmap_functional::observations`. */
static void
observe (const gchar *key,
         const gchar *value)
{
	g_print ("%s=%s\n", key, value);
}

static void
observe_boolean (const gchar *key,
                 gboolean value)
{
	observe (key, value ? "1" : "0");
}

/* Fail the run and name the failed step. Use only for preconditions of this
 * program. Report the behaviour under test, do not judge it here. */
static void G_GNUC_NORETURN
fatal (const gchar *what,
       const GError *error)
{
	g_printerr ("%s: %s\n", what, error ? error->message : "(no error set)");
	exit (1);
}

/* Call `method` on the bus daemon for `unique_name`. Returns the reply, or
 * NULL with `error` set (for example `NameHasNoOwner` after the peer stops).
 *
 * This calls the bus daemon, not the well-known name of the registry. A call
 * to `org.gnome.evolution.dataserver.Sources5` would D-Bus-ACTIVATE a new
 * registry, which must not occur between the kill and the token fetch. A
 * unique name cannot be activated. */
static GVariant *
ask_bus_about (GDBusConnection *connection,
               const gchar *method,
               const gchar *unique_name,
               GError **error)
{
	return g_dbus_connection_call_sync (
		connection,
		"org.freedesktop.DBus",
		"/org/freedesktop/DBus",
		"org.freedesktop.DBus",
		method,
		g_variant_new ("(s)", unique_name),
		NULL, G_DBUS_CALL_FLAGS_NO_AUTO_START, -1, NULL, error);
}

/* TRUE once `unique_name` has no owner on the bus. */
static gboolean
name_is_gone (GDBusConnection *connection,
              const gchar *unique_name)
{
	GError *error = NULL;
	GVariant *reply;

	reply = ask_bus_about (connection, "GetNameOwner", unique_name, &error);

	if (reply != NULL) {
		g_variant_unref (reply);

		return FALSE;
	}

	g_clear_error (&error);

	return TRUE;
}

/* TRUE if the D-Bus object of `source` has the OAuth2Support interface now.
 * Ask the source again each time: a pointer kept from an earlier call does
 * not show a later change. */
static gboolean
source_exports_oauth2_support (ESource *source)
{
	GDBusObject *dbus_object;
	GDBusInterface *dbus_interface = NULL;
	gboolean exported;

	dbus_object = e_source_ref_dbus_object (source);
	if (dbus_object != NULL)
		dbus_interface = g_dbus_object_get_interface (dbus_object, OAUTH2_SUPPORT_INTERFACE);

	exported = dbus_interface != NULL;

	g_clear_object (&dbus_interface);
	g_clear_object (&dbus_object);

	return exported;
}

int
main (int argc,
      char **argv)
{
	ESourceRegistry *registry;
	ESource *source;
	GDBusObject *dbus_object;
	GDBusInterface *dbus_interface;
	GDBusConnection *connection;
	GVariant *reply;
	GError *error = NULL;
	const gchar *account_uid;
	gchar *peer = NULL;
	gchar *access_token = NULL;
	gint expires_in = 0;
	guint32 registry_pid = 0;
	gint64 deadline;
	gboolean fetched;

	if (argc != 2) {
		g_printerr ("usage: %s <account-uid>\n", argv[0]);

		return 2;
	}

	account_uid = argv[1];

	/* This starts evolution-source-registry on the private session bus. It
	 * loads all modules in EDS_REGISTRY_MODULES: here module-jmap-backend.so
	 * (EOAuth2Service "JMAP") and EDS module-oauth2-services.so (the monitor
	 * that exports the interface). */
	registry = e_source_registry_new_sync (NULL, &error);
	if (registry == NULL)
		fatal ("e_source_registry_new_sync", error);

	source = e_source_registry_ref_source (registry, account_uid);
	observe_boolean ("source-found", source != NULL);
	if (source == NULL) {
		g_printerr ("the registry does not know the account %s\n", account_uid);

		return 1;
	}

	/* The necessary observation. Non-NULL means that the registry exports
	 * Source.OAuth2Support for this account. Thus a token fetch goes over the
	 * bus, not through the in-process EOAuth2Services fallback in e-source.c. */
	dbus_object = e_source_ref_dbus_object (source);
	dbus_interface = dbus_object != NULL
		? g_dbus_object_get_interface (dbus_object, OAUTH2_SUPPORT_INTERFACE)
		: NULL;
	g_clear_object (&dbus_object);

	observe_boolean ("oauth2-support-exported", dbus_interface != NULL);

	if (dbus_interface == NULL) {
		/* With no interface, there is no proxy to become stale and nothing
		 * more to measure. The Rust side reports the failure. */
		return 0;
	}

	/* The proxy peer is the *unique* name of the registry (see the assertion
	 * in gdbusobjectmanagerclient.c). The live failure called it ":1.4". */
	if (!G_IS_DBUS_PROXY (dbus_interface)) {
		g_printerr ("the OAuth2Support interface is not a proxy; "
			"this program is meant to run as a registry *client*\n");

		return 1;
	}

	peer = g_strdup (g_dbus_proxy_get_name (G_DBUS_PROXY (dbus_interface)));
	observe ("oauth2-support-peer", peer);
	observe_boolean ("oauth2-support-peer-is-unique-name", g_dbus_is_unique_name (peer));

	connection = g_dbus_proxy_get_connection (G_DBUS_PROXY (dbus_interface));

	/* A token fetch BEFORE the restart shows that the proxy works at first.
	 * Thus a later failure comes from the restart. The fetch FAILS (this
	 * scratch session has no refresh token), but the error comes from the
	 * JMAP EOAuth2Service through a live registry, not from the bus. The
	 * Rust side checks that the error domain is not a D-Bus failure. */
	fetched = e_source_get_oauth2_access_token_sync (
		source, NULL, &access_token, &expires_in, &error);
	observe_boolean ("token-before-kill-succeeded", fetched);
	observe ("token-before-kill-error-domain",
		error != NULL ? g_quark_to_string (error->domain) : "");
	observe_boolean ("token-before-kill-was-bus-error",
		error != NULL && error->domain == G_DBUS_ERROR);
	g_clear_pointer (&access_token, g_free);
	g_clear_error (&error);

	reply = ask_bus_about (connection, "GetConnectionUnixProcessID", peer, &error);
	if (reply == NULL)
		fatal ("GetConnectionUnixProcessID for the registry", error);
	g_variant_get (reply, "(u)", &registry_pid);
	g_variant_unref (reply);

	/* Use SIGKILL, not SIGTERM. On a clean stop, the registry unexports its
	 * objects and the client removes the interface: then the bug does NOT
	 * occur. The live capture was a replaced or crashed instance. */
	if (kill ((pid_t) registry_pid, SIGKILL) != 0) {
		g_printerr ("kill(%u, SIGKILL): %s\n", registry_pid, g_strerror (errno));

		return 1;
	}

	deadline = g_get_monotonic_time () + NAME_GONE_TIMEOUT_SECONDS * G_TIME_SPAN_SECOND;
	while (!name_is_gone (connection, peer) && g_get_monotonic_time () < deadline)
		g_usleep (10 * 1000);

	observe_boolean ("registry-name-gone", name_is_gone (connection, peer));

	/* The reproduction: the same ESource, with its interface proxy to a name
	 * that has no owner. */
	access_token = NULL;
	expires_in = 0;
	fetched = e_source_get_oauth2_access_token_sync (
		source, NULL, &access_token, &expires_in, &error);

	observe_boolean ("token-after-kill-succeeded", fetched);
	observe_boolean ("oauth2-support-still-exported", source_exports_oauth2_support (source));

	if (error != NULL) {
		gchar *code = g_strdup_printf ("%d", error->code);

		observe ("token-after-kill-error-domain", g_quark_to_string (error->domain));
		observe ("token-after-kill-error-code", code);
		/* e-source.c already applied g_dbus_error_strip_remote_error. Thus
		 * this is the message that jmap_backend_core::oauth2 classifies and
		 * that the user sees. */
		observe ("token-after-kill-error-message", error->message);
		observe_boolean ("token-after-kill-is-service-unknown",
			g_error_matches (error, G_DBUS_ERROR, G_DBUS_ERROR_SERVICE_UNKNOWN));
		observe_boolean ("token-after-kill-names-dead-peer",
			strstr (error->message, peer) != NULL);

		g_free (code);
	}

	g_clear_pointer (&access_token, g_free);
	g_clear_error (&error);
	g_free (peer);
	g_object_unref (dbus_interface);
	g_object_unref (source);
	g_object_unref (registry);

	return 0;
}
