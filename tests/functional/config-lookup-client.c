/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Loads module-jmap-configuration.so itself, because no daemon loads it
 * (see the doc comment of jmap_config::module). Then it makes a real
 * EConfigLookup with a real ESourceRegistry and runs JmapConfigLookup, as the
 * "Look Up Account Details" step of the account assistant does.
 *
 * `rust/crates/jmap-functional/tests/config-lookup.rs` runs this program and
 * does the assertions.
 *
 *   usage: functional-config-lookup-client <module-dir> <email-address> [servers]
 */

#include <e-util/e-util.h>

typedef struct {
	GMainLoop *loop;
	gboolean finished;
	gboolean worker_finished_seen;
	GError *worker_error;
} RunState;

static void
run_finished_cb (GObject *source_object,
                  GAsyncResult *result,
                  gpointer user_data)
{
	RunState *state = user_data;
	EConfigLookup *config_lookup = E_CONFIG_LOOKUP (source_object);

	if (result)
		e_config_lookup_run_finish (config_lookup, result);

	state->finished = TRUE;
	g_main_loop_quit (state->loop);
}

/* `e_config_lookup_run_finish()` returns void (see upstream
 * `e-config-lookup.c`). Thus the GError of a worker comes only through this
 * signal. */
static void
worker_finished_cb (EConfigLookup *config_lookup,
                     EConfigLookupWorker *worker,
                     ENamedParameters *restart_params,
                     GError *error,
                     gpointer user_data)
{
	RunState *state = user_data;

	state->worker_finished_seen = TRUE;
	g_clear_error (&state->worker_error);
	if (error)
		state->worker_error = g_error_copy (error);
}

static gboolean
timeout_cb (gpointer user_data)
{
	RunState *state = user_data;

	g_printerr ("config-lookup-client: timed out waiting for e_config_lookup_run\n");
	g_main_loop_quit (state->loop);

	return G_SOURCE_REMOVE;
}

static int
fail (const gchar *step,
      GError *error)
{
	g_printerr ("%s: %s\n", step, error ? error->message : "(no error set)");
	g_clear_error (&error);

	return 1;
}

int
main (int argc,
      char **argv)
{
	GError *error = NULL;
	GList *modules, *link;
	ESourceRegistry *registry;
	EConfigLookup *config_lookup;
	ENamedParameters *params;
	RunState state;
	GSource *timeout_source;
	GSList *results;
	const gchar *module_dir;
	const gchar *email_address;
	const gchar *servers;

	if (argc < 3 || argc > 4) {
		g_printerr ("usage: %s <module-dir> <email-address> [servers]\n", argv[0]);
		return 2;
	}

	module_dir = argv[1];
	email_address = argv[2];
	servers = argc > 3 ? argv[3] : NULL;

	/* Loads the modules and registers their types, as the Evolution shell
	 * does at startup. g_type_module_use() makes EModule call e_module_load. */
	modules = e_module_load_all_in_directory (module_dir);
	if (!modules) {
		g_printerr ("no modules found in %s\n", module_dir);
		return 1;
	}
	for (link = modules; link; link = g_list_next (link))
		g_type_module_use (G_TYPE_MODULE (link->data));

	/* Starts evolution-source-registry. e_config_lookup_new() refuses a NULL
	 * registry. */
	registry = e_source_registry_new_sync (NULL, &error);
	if (!registry)
		return fail ("registry", error);

	/* The constructor of EConfigLookup calls e_extensible_load_extensions(),
	 * which creates JmapConfigLookup. See jmap_config::module. */
	config_lookup = e_config_lookup_new (registry);

	params = e_named_parameters_new ();
	e_named_parameters_set (params, E_CONFIG_LOOKUP_PARAM_EMAIL_ADDRESS, email_address);
	if (servers && *servers)
		e_named_parameters_set (params, E_CONFIG_LOOKUP_PARAM_SERVERS, servers);

	state.loop = g_main_loop_new (NULL, FALSE);
	state.finished = FALSE;
	state.worker_finished_seen = FALSE;
	state.worker_error = NULL;

	/* Upstream emits this signal in the main thread, so a connection before
	 * the run has no race with the worker thread. */
	g_signal_connect (config_lookup, "worker-finished", G_CALLBACK (worker_finished_cb), &state);

	timeout_source = g_timeout_source_new_seconds (30);
	g_source_set_callback (timeout_source, timeout_cb, &state, NULL);
	g_source_attach (timeout_source, NULL);

	e_config_lookup_run (config_lookup, params, NULL, run_finished_cb, &state);
	g_main_loop_run (state.loop);

	g_source_destroy (timeout_source);
	g_source_unref (timeout_source);
	g_main_loop_unref (state.loop);
	e_named_parameters_free (params);

	if (!state.finished) {
		g_printerr ("e_config_lookup_run never finished\n");
		return 1;
	}

	g_print ("worker-finished-seen=%d\n", state.worker_finished_seen ? 1 : 0);
	if (state.worker_error)
		g_print ("worker-error-message=%s\n", state.worker_error->message);
	g_clear_error (&state.worker_error);

	results = e_config_lookup_dup_results (config_lookup, E_CONFIG_LOOKUP_RESULT_COLLECTION, "jmap");
	g_print ("result-count=%d\n", g_slist_length (results));

	if (results) {
		EConfigLookupResult *lookup_result = E_CONFIG_LOOKUP_RESULT (results->data);
		ESource *source;
		ESourceExtension *extension;

		g_print ("protocol=%s\n", e_config_lookup_result_get_protocol (lookup_result));
		g_print ("display-name=%s\n", e_config_lookup_result_get_display_name (lookup_result));
		g_print ("is-complete=%d\n", e_config_lookup_result_get_is_complete (lookup_result) ? 1 : 0);

		/* Applies the values of JmapConfigLookup::add_result() to a scratch
		 * source, as the account assistant does. This is the only way to read
		 * them: e-config-lookup-result-simple.c has no getters. */
		source = e_source_new (NULL, NULL, &error);
		if (!source)
			return fail ("source-new", error);

		if (!e_config_lookup_result_configure_source (lookup_result, config_lookup, source)) {
			g_printerr ("configure-source failed\n");
			return 1;
		}

		extension = e_source_get_extension (source, E_SOURCE_EXTENSION_COLLECTION);
		g_print ("collection-backend-name=%s\n", e_source_backend_get_backend_name (E_SOURCE_BACKEND (extension)));
		g_print ("collection-identity=%s\n", e_source_collection_get_identity (E_SOURCE_COLLECTION (extension)));

		extension = e_source_get_extension (source, E_SOURCE_EXTENSION_AUTHENTICATION);
		g_print ("authentication-host=%s\n", e_source_authentication_get_host (E_SOURCE_AUTHENTICATION (extension)));
		g_print ("authentication-port=%u\n", (guint) e_source_authentication_get_port (E_SOURCE_AUTHENTICATION (extension)));
		g_print ("authentication-user=%s\n", e_source_authentication_get_user (E_SOURCE_AUTHENTICATION (extension)));
		g_print ("authentication-method=%s\n", e_source_authentication_get_method (E_SOURCE_AUTHENTICATION (extension)));

		extension = e_source_get_extension (source, E_SOURCE_EXTENSION_SECURITY);
		g_print ("security-method=%s\n", e_source_security_get_method (E_SOURCE_SECURITY (extension)));

		g_object_unref (source);
		g_slist_free_full (results, g_object_unref);
	}

	g_object_unref (config_lookup);
	g_object_unref (registry);

	return 0;
}
