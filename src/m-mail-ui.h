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

#ifndef M_MAIL_UI_H
#define M_MAIL_UI_H

#include <gtk/gtk.h>
#include <shell/e-shell-view.h>

#if JMAP_EVO_EUI_MANAGER
#define __E_UTIL_H_INSIDE__
#include <e-util/e-ui-manager.h>
#endif

G_BEGIN_DECLS

#if JMAP_EVO_EUI_MANAGER
void		m_mail_ui_init		(EUIManager *ui_manager,
					 EShellView *shell_view);
#else
void		m_mail_ui_init		(GtkUIManager *ui_manager,
					 EShellView *shell_view,
					 gchar **ui_definition);
#endif

G_END_DECLS

#endif /* M_MAIL_UI_H */
