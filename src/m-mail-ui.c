/*
 * Copyright (C) 2016 Red Hat, Inc. (www.redhat.com)
 *
 * This library is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Lesser General Public License as published by
 * the Free Software Foundation.
 *
 * This library is distributed in the hope that it will be useful, but
 * WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
 * or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Lesser General Public License
 * for more details.
 *
 * You should have received a copy of the GNU Lesser General Public License
 * along with this library. If not, see <http://www.gnu.org/licenses/>.
 */

#ifdef HAVE_CONFIG_H
#include "config.h"
#endif

#include <glib/gi18n-lib.h>

#include <gtk/gtk.h>
#include <shell/e-shell-view.h>

#include <mail/em-folder-tree.h>
#include <mail/e-mail-reader.h>
#include <mail/e-mail-view.h>
#include <mail/e-mail-paned-view.h>
#include <mail/message-list.h>

#include "m-utils.h"
#include "m-mail-ui.h"

#define REQUIRE_SERVICE_PROTOCOL "maildir"

static gboolean
get_store_from_folder_tree (EShellView *shell_view,
			    gchar **pfolder_path,
			    CamelStore **pstore)
{
	EShellSidebar *shell_sidebar;
	EMFolderTree *folder_tree;
	gchar *selected_path = NULL;
	CamelStore *selected_store = NULL;
	gboolean found = FALSE;

	/* Get hold of Folder Tree */
	shell_sidebar = e_shell_view_get_shell_sidebar (shell_view);
	g_object_get (shell_sidebar, "folder-tree", &folder_tree, NULL);
	if (em_folder_tree_get_selected (folder_tree, &selected_store, &selected_path) ||
	    em_folder_tree_store_root_selected (folder_tree, &selected_store)) {
		if (selected_store) {
			CamelProvider *provider = camel_service_get_provider (CAMEL_SERVICE (selected_store));

			if (provider && g_ascii_strcasecmp (provider->protocol, REQUIRE_SERVICE_PROTOCOL) == 0) {
				found = TRUE;

				if (pstore)
					*pstore = g_object_ref (selected_store);

				if (pfolder_path)
					*pfolder_path = selected_path;
				else
					g_free (selected_path);

				selected_path = NULL;
			}

			g_object_unref (selected_store);
		}

		g_free (selected_path);
	}

	g_object_unref (folder_tree);

	return found;
}

static void
do_action_mail_folder (EShellView *shell_view)
{
	gchar *folder_path = NULL;
	CamelStore *store = NULL;

	g_return_if_fail (E_IS_SHELL_VIEW (shell_view));

	if (!get_store_from_folder_tree (shell_view, &folder_path, &store))
		return;

	g_print ("%s: Executed on folder '%s : %s'\n", G_STRFUNC, camel_service_get_display_name (CAMEL_SERVICE (store)), folder_path);

	g_free (folder_path);
	g_clear_object (&store);
}

static void
do_action_mail_message (EShellView *shell_view)
{
	EShellContent *shell_content;
	EMailView *mail_view = NULL;
	GPtrArray *selected_uids = NULL;
	CamelFolder *folder = NULL;
	guint ii;

	g_return_if_fail (E_IS_SHELL_VIEW (shell_view));

	shell_content = e_shell_view_get_shell_content (shell_view);
	g_object_get (shell_content, "mail-view", &mail_view, NULL);
	if (E_IS_MAIL_PANED_VIEW (mail_view)) {
		selected_uids = e_mail_reader_get_selected_uids (E_MAIL_READER (mail_view));
		folder = e_mail_reader_ref_folder (E_MAIL_READER (mail_view));
	}

	g_print ("%s: Folder '%s' has selected %d messages:\n", G_STRFUNC,
		folder ? camel_folder_get_full_name (folder) : "[null]",
		selected_uids ? selected_uids->len : -1);

	for (ii = 0; selected_uids && ii < selected_uids->len; ii++) {
		g_print ("   %s\n", (const gchar *) g_ptr_array_index (selected_uids, ii));
	}

	if (selected_uids)
		g_ptr_array_unref (selected_uids);
	g_clear_object (&folder);
}

/* folder_node/has_message: the same two gates the pre-3.55 code computed,
 * shared verbatim by both menu layers' own update-actions handler. */
static void
mail_update_state (EShellView *shell_view,
		   gboolean *folder_node,
		   gboolean *has_message)
{
	EShellSidebar *shell_sidebar;
	EShellContent *shell_content;
	EMFolderTree *folder_tree;
	EMailView *mail_view = NULL;
	CamelStore *selected_store = NULL;
	gchar *selected_path = NULL;
	gboolean account_node = FALSE;

	*folder_node = FALSE;
	*has_message = FALSE;

	shell_sidebar = e_shell_view_get_shell_sidebar (shell_view);
	g_object_get (shell_sidebar, "folder-tree", &folder_tree, NULL);
	if (em_folder_tree_get_selected (folder_tree, &selected_store, &selected_path) ||
	    em_folder_tree_store_root_selected (folder_tree, &selected_store)) {
		if (selected_store) {
			CamelProvider *provider = camel_service_get_provider (CAMEL_SERVICE (selected_store));

			if (provider && g_ascii_strcasecmp (provider->protocol, REQUIRE_SERVICE_PROTOCOL) == 0) {
				account_node = !selected_path || !*selected_path;
				*folder_node = !account_node;
			}

			g_object_unref (selected_store);
		}
	}
	g_object_unref (folder_tree);
	g_free (selected_path);

	/* To get to messages in the separate window (those when double-cliecked in the message list),
	   a new extension extending E_TYPE_MAIL_BROWSER is required. The EMailBrowser implements
	   EMailReader, thus it's easier to get to the message list there. */
	shell_content = e_shell_view_get_shell_content (shell_view);
	g_object_get (shell_content, "mail-view", &mail_view, NULL);
	if (E_IS_MAIL_PANED_VIEW (mail_view)) {
		GtkWidget *message_list;

		message_list = e_mail_reader_get_message_list (E_MAIL_READER (mail_view));
		*has_message = message_list_selected_count (MESSAGE_LIST (message_list)) > 0;
	}
}

#if JMAP_EVO_EUI_MANAGER

/* This module's own group, not one of Evolution's own (the same choice
 * jmap-ui's snooze/send_later extensions make): action references inside the
 * merged <eui> resolve across the whole manager, not just this group, so
 * nothing about placement depends on the name. */
#define GROUP_NAME "example-module-mail"

static void
action_mail_folder_cb (EUIAction *action,
		       GVariant *value,
		       gpointer user_data)
{
	do_action_mail_folder (E_SHELL_VIEW (user_data));
}

static void
action_mail_message_cb (EUIAction *action,
			GVariant *value,
			gpointer user_data)
{
	do_action_mail_message (E_SHELL_VIEW (user_data));
}

/* Index 0 is the folder-popup item, gated on folder_node; index 1 is the
 * message-menu item, gated on has_message — matching the pre-3.55 code's own
 * two calls in m_mail_ui_update_actions_cb. */
static const EUIActionEntry mail_entries[] = {
	{ "my-mail-ui-folder-action",
	  "folder-new",
	  N_("M_y Maildir Folder Action..."),
	  NULL,
	  N_("My Maildir Folder Action"),
	  action_mail_folder_cb, NULL, NULL, NULL },
	{ "my-mail-ui-message-action",
	  "document-new",
	  N_("M_y Message Action..."),
	  NULL,
	  N_("My Message Action"),
	  action_mail_message_cb, NULL, NULL, NULL }
};

static void
m_mail_ui_update_actions_cb (EShellView *shell_view,
			     gpointer user_data)
{
	EUIManager *ui_manager;
	EUIActionGroup *action_group;
	gboolean folder_node, has_message;

	ui_manager = e_shell_view_get_ui_manager (shell_view);
	action_group = e_ui_manager_get_action_group (ui_manager, GROUP_NAME);
	if (!action_group)
		return;

	mail_update_state (shell_view, &folder_node, &has_message);

	m_utils_enable_actions (action_group, &mail_entries[0], 1, folder_node);
	m_utils_enable_actions (action_group, &mail_entries[1], 1, has_message);
}

void
m_mail_ui_init (EUIManager *ui_manager,
		EShellView *shell_view)
{
	/* 3.52 merged the same pair (an item in the shell's own
	 * 'mail-message-menu' submenu's own 'mail-message-custom-menus'
	 * placeholder, an item in 'mail-folder-popup's own
	 * 'mail-folder-popup-actions' placeholder) as a GtkUIManager string;
	 * the ids and nesting here are unchanged, confirmed against this
	 * build's own evolution-mail.eui and evolution-mail-reader.eui —
	 * only the wrapping element and the name/id, menu/submenu,
	 * menuitem/item spelling differ. */
	const gchar *ui_def =
		"<eui>"
		"<menu id='main-menu'>"
		"<placeholder id='custom-menus'>"
		"<submenu action='mail-message-menu'>"
		"<placeholder id='mail-message-custom-menus'>"
		"<item action='my-mail-ui-message-action'/>"
		"</placeholder>"
		"</submenu>"
		"</placeholder>"
		"</menu>"
		"<menu id='mail-folder-popup'>"
		"<placeholder id='mail-folder-popup-actions'>"
		"<item action='my-mail-ui-folder-action'/>"
		"</placeholder>"
		"</menu>"
		"</eui>";

	g_return_if_fail (ui_manager != NULL);
	g_return_if_fail (shell_view != NULL);

	if (e_ui_manager_get_action_group (ui_manager, GROUP_NAME))
		return;

	e_ui_manager_add_actions_with_eui_data (
		ui_manager, GROUP_NAME, GETTEXT_PACKAGE,
		mail_entries, G_N_ELEMENTS (mail_entries),
		shell_view, ui_def);

	g_signal_connect (
		shell_view, "update-actions",
		G_CALLBACK (m_mail_ui_update_actions_cb),
		NULL);
}

#else /* JMAP_EVO_EUI_MANAGER */

static void
action_mail_folder_cb (GtkAction *action,
		       EShellView *shell_view)
{
	do_action_mail_folder (shell_view);
}

static void
action_mail_message_cb (GtkAction *action,
			EShellView *shell_view)
{
	do_action_mail_message (shell_view);
}

static GtkActionEntry mail_folder_context_entries[] = {
	{ "my-mail-ui-folder-action",
	  "folder-new",
	  N_("M_y Maildir Folder Action..."),
	  NULL,
	  N_("My Maildir Folder Action"),
	  G_CALLBACK (action_mail_folder_cb) }
};

static GtkActionEntry mail_message_menu_entries[] = {
	{ "my-mail-ui-message-action",
	  "document-new",
	  N_("M_y Message Action..."),
	  NULL,
	  N_("My Message Action"),
	  G_CALLBACK (action_mail_message_cb) }
};

static void
m_mail_ui_update_actions_cb (EShellView *shell_view,
			     GtkActionEntry *entries)
{
	EShellWindow *shell_window;
	GtkActionGroup *action_group;
	GtkUIManager *ui_manager;
	gboolean folder_node, has_message;

	mail_update_state (shell_view, &folder_node, &has_message);

	shell_window = e_shell_view_get_shell_window (shell_view);
	ui_manager = e_shell_window_get_ui_manager (shell_window);
	action_group = e_lookup_action_group (ui_manager, "mail");

	m_utils_enable_actions (action_group, mail_folder_context_entries, G_N_ELEMENTS (mail_folder_context_entries), folder_node);
	m_utils_enable_actions (action_group, mail_message_menu_entries, G_N_ELEMENTS (mail_message_menu_entries), has_message);
}

void
m_mail_ui_init (GtkUIManager *ui_manager,
		EShellView *shell_view,
		gchar **ui_definition)
{
	const gchar *ui_def =
		"<menubar name='main-menu'>\n"
		"  <placeholder name='custom-menus'>\n"
		"    <menu action='mail-message-menu'>\n"
		"      <placeholder name='mail-message-custom-menus'>\n"
		"        <menuitem action=\"my-mail-ui-message-action\"/>\n"
		"      </placeholder>\n"
		"    </menu>\n"
		"  </placeholder>\n"
		"</menubar>\n"
		"\n"
		"<popup name=\"mail-folder-popup\">\n"
		"  <placeholder name=\"mail-folder-popup-actions\">\n"
		"    <menuitem action=\"my-mail-ui-folder-action\"/>\n"
		"  </placeholder>\n"
		"</popup>\n";

	EShellWindow *shell_window;
	GtkActionGroup *action_group;

	g_return_if_fail (ui_definition != NULL);

	*ui_definition = g_strdup (ui_def);

	shell_window = e_shell_view_get_shell_window (shell_view);
	action_group = e_shell_window_get_action_group (shell_window, "mail");

	/* Add actions to the action group. */
	e_action_group_add_actions_localized (
		action_group, GETTEXT_PACKAGE,
		mail_folder_context_entries, G_N_ELEMENTS (mail_folder_context_entries), shell_view);
	e_action_group_add_actions_localized (
		action_group, GETTEXT_PACKAGE,
		mail_message_menu_entries, G_N_ELEMENTS (mail_message_menu_entries), shell_view);

	/* Decide whether we want this option to be visible or not */
	g_signal_connect (
		shell_view, "update-actions",
		G_CALLBACK (m_mail_ui_update_actions_cb),
		shell_view);
}

#endif /* JMAP_EVO_EUI_MANAGER */
