/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The client half of `docs/ROADMAP.md` item 22's Do(1): the headless
 * reproduction of the stale `Source.OAuth2Support` interface proxy that turns
 * a silent token fetch into `G_DBUS_ERROR_SERVICE_UNKNOWN` — the failure the
 * item-20 tracing captured live on 2026-08-28 ("The name :1.4 was not
 * provided by any .service files"), which then escalated to a consent window.
 *
 * This program is an ordinary `libedataserver` consumer standing in for a
 * calendar or address-book *factory* process: it holds an `ESource` across a
 * registry restart, exactly as a long-lived factory does, and asks it for an
 * OAuth 2.0 access token afterwards. It knows nothing about JMAP beyond the
 * account uid it is given, and asserts nothing — every judgement belongs to
 * `rust/crates/jmap-functional/tests/oauth2-stale-proxy.rs`, which runs this
 * program and reads the `key=value` observations it writes to stdout.
 *
 * WHY THIS REPRODUCES THE REAL THING, and not merely D-Bus's own semantics
 * (EDS 3.52.3 / Evolution 3.52.4 source, read 2026-08-29):
 *
 *   1. It is the *registry* that exports the interface, not Evolution's
 *      shell. `grep -rn OAuth2Support` over all of evolution-3.52.4 finds one
 *      file, a mail-config summary widget; the shell exports no such object.
 *      `e_server_side_source_set_oauth2_support`'s own doc says who does:
 *      "If @oauth2_support is non-NULL, the OAuth2Support D-Bus interface is
 *      exported at the object path for @source."
 *   2. Our accounts reach that setter through EDS's own registry module
 *      `module-oauth2-services.c:139`, whose `EOAuth2SourceMonitor` calls it
 *      for every server-side source whose `[Authentication] Method` is an
 *      OAuth2 alias — i.e. names a registered `EOAuth2Service`. Ours is
 *      `jmap_config::oauth2_service::NAME`, the string "JMAP". That module
 *      must therefore be staged beside `module-jmap-backend.so`, because
 *      `EDS_REGISTRY_MODULES` *replaces* EDS's module directory rather than
 *      adding to it (`e-source-registry-server.c:1073`).
 *   3. Client-side, `e-source.c::source_get_oauth2_access_token_sync` takes
 *      its in-process `e_oauth2_services_find` fallback only when the D-Bus
 *      interface is ABSENT. When it is present the call goes over the bus,
 *      through a proxy `GDBusObjectManagerClient` addressed to the manager's
 *      *unique* name (`gdbusobjectmanagerclient.c` sets "g-name" to
 *      `name_owner`, asserted `g_dbus_is_unique_name`).
 *   4. The staleness is deterministic, not a race. When the registry dies,
 *      `source_registry_object_removed_cb` sees a NULL name-owner and takes
 *      `source_registry_object_removed_no_owner`, which only forgets the
 *      object path — it does NOT call `__e_source_private_replace_dbus_object
 *      (source, NULL)`. That happens only in the *by_owner* branch, i.e. when
 *      a source is genuinely deleted while the service is alive. So an
 *      `ESource` a factory already holds keeps its dead proxy indefinitely,
 *      and no amount of main-loop iteration clears it while the name has no
 *      owner.
 *
 * Steps 1 and 3 are what this program observes directly, and step 1 is the
 * load-bearing one: `oauth2-support-exported` is the fact an earlier session
 * doubted (it concluded the whole token path was D-Bus-free), and every other
 * observation here is worthless without it.
 *
 *   usage: functional-oauth2-stale-proxy-client <account-uid>
 */

#include <libedataserver/libedataserver.h>
#include <signal.h>
#include <string.h>

/* The interface `e_server_side_source_set_oauth2_support` exports, named as a
 * string rather than reached through EDS's generated `EDBusSourceOAuth2Support`
 * accessors: those live in `src/private/e-dbus-source.h`, which EDS does not
 * install. `e_source_ref_dbus_object` is public and returns a plain
 * `GDBusObject`, so `g_dbus_object_get_interface` asks the same question of it
 * without a private header. From `src/private/
 * org.gnome.evolution.dataserver.Source.xml:210`. */
#define OAUTH2_SUPPORT_INTERFACE "org.gnome.evolution.dataserver.Source.OAuth2Support"

/* How long to wait for the killed registry's unique name to actually leave
 * the bus. SIGKILL is immediate but the daemon's disconnection reaches the
 * bus asynchronously; a limit at all is so that a registry which somehow
 * survives fails the run instead of hanging it. */
#define NAME_GONE_TIMEOUT_SECONDS 10

/* One `key=value` observation on stdout, the format `jmap_functional::
 * observations` parses. */
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

/* Fail the run, naming what broke. Used only for things that are this
 * program's own preconditions rather than the behaviour under test — the
 * behaviour under test is always *reported*, never judged here. */
static void G_GNUC_NORETURN
fatal (const gchar *what,
       const GError *error)
{
	g_printerr ("%s: %s\n", what, error ? error->message : "(no error set)");
	exit (1);
}

/* Ask the bus daemon itself a question about `unique_name`. Returns the
 * reply, or NULL with `error` set — notably `NameHasNoOwner` once the peer
 * is gone, which is what this program waits for.
 *
 * Deliberately a direct `g_dbus_connection_call_sync` on the bus daemon
 * rather than anything on the registry's own well-known name: asking for
 * `org.gnome.evolution.dataserver.Sources5` would D-Bus-ACTIVATE a fresh
 * registry, which is precisely what must not happen between the kill and the
 * token fetch. A unique name can never be activated. */
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

	/* Activates evolution-source-registry on this session's private bus,
	 * which loads every module in EDS_REGISTRY_MODULES — for this test,
	 * module-jmap-backend.so (our EOAuth2Service "JMAP") and EDS's own
	 * module-oauth2-services.so (the monitor that exports the interface). */
	registry = e_source_registry_new_sync (NULL, &error);
	if (registry == NULL)
		fatal ("e_source_registry_new_sync", error);

	source = e_source_registry_ref_source (registry, account_uid);
	observe_boolean ("source-found", source != NULL);
	if (source == NULL) {
		g_printerr ("the registry does not know the account %s\n", account_uid);

		return 1;
	}

	/* THE load-bearing observation. Non-NULL means the registry really did
	 * export Source.OAuth2Support for this account, so a token fetch from
	 * this process goes over the bus rather than through e-source.c's
	 * in-process EOAuth2Services fallback. */
	dbus_object = e_source_ref_dbus_object (source);
	dbus_interface = dbus_object != NULL
		? g_dbus_object_get_interface (dbus_object, OAUTH2_SUPPORT_INTERFACE)
		: NULL;
	g_clear_object (&dbus_object);

	observe_boolean ("oauth2-support-exported", dbus_interface != NULL);

	if (dbus_interface == NULL) {
		/* Not this program's business to judge, but there is nothing
		 * further it can measure either: with no interface there is no
		 * proxy to go stale. The Rust side reports why this matters. */
		return 0;
	}

	/* The proxy's peer: the registry's *unique* name, per
	 * gdbusobjectmanagerclient.c's own "use a unique name" assertion. This
	 * is the string the captured live failure called ":1.4". */
	if (!G_IS_DBUS_PROXY (dbus_interface)) {
		g_printerr ("the OAuth2Support interface is not a proxy; "
			"this program is meant to run as a registry *client*\n");

		return 1;
	}

	peer = g_strdup (g_dbus_proxy_get_name (G_DBUS_PROXY (dbus_interface)));
	observe ("oauth2-support-peer", peer);
	observe_boolean ("oauth2-support-peer-is-unique-name", g_dbus_is_unique_name (peer));

	connection = g_dbus_proxy_get_connection (G_DBUS_PROXY (dbus_interface));

	/* A token fetch BEFORE the restart, to prove the proxy is live to begin
	 * with and that what follows is the restart's doing rather than a token
	 * path that never worked. It is expected to FAIL — there is no stored
	 * refresh token in this scratch session — but with a failure from our
	 * own EOAuth2Service reached through a live registry, not with a bus
	 * error. So what is reported is the error's domain, and the Rust side
	 * holds it to "not a D-Bus transport failure". */
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

	/* SIGKILL rather than SIGTERM: a registry given the chance to shut down
	 * cleanly unexports its objects first, and a client that saw the
	 * unexport would drop the interface — which is the one path that does
	 * NOT reproduce the bug. The live capture was a replaced/crashed
	 * instance, so an abrupt death is the faithful reproduction. */
	if (kill ((pid_t) registry_pid, SIGKILL) != 0) {
		g_printerr ("kill(%u, SIGKILL): %s\n", registry_pid, g_strerror (errno));

		return 1;
	}

	deadline = g_get_monotonic_time () + NAME_GONE_TIMEOUT_SECONDS * G_TIME_SPAN_SECOND;
	while (!name_is_gone (connection, peer) && g_get_monotonic_time () < deadline)
		g_usleep (10 * 1000);

	observe_boolean ("registry-name-gone", name_is_gone (connection, peer));

	/* The reproduction itself: the very same ESource, still held, still
	 * carrying the interface proxy addressed to a name nothing owns. */
	access_token = NULL;
	expires_in = 0;
	fetched = e_source_get_oauth2_access_token_sync (
		source, NULL, &access_token, &expires_in, &error);

	observe_boolean ("token-after-kill-succeeded", fetched);
	observe_boolean ("oauth2-support-still-exported", dbus_interface != NULL);

	if (error != NULL) {
		gchar *code = g_strdup_printf ("%d", error->code);

		observe ("token-after-kill-error-domain", g_quark_to_string (error->domain));
		observe ("token-after-kill-error-code", code);
		/* e-source.c already applied g_dbus_error_strip_remote_error to
		 * this message before propagating it, so what is reported here is
		 * exactly what jmap_backend_core::oauth2 classifies and what the
		 * user would be shown. */
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
