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

#ifndef M_UTILS_H
#define M_UTILS_H

#include <gtk/gtk.h>

#if JMAP_EVO_EUI_MANAGER
#define __E_UTIL_H_INSIDE__
#include <e-util/e-ui-action-group.h>
#endif

G_BEGIN_DECLS

#if JMAP_EVO_EUI_MANAGER
void		m_utils_enable_actions		(EUIActionGroup *action_group,
						 const EUIActionEntry *entries,
						 guint n_entries,
						 gboolean enable);
#else
void		m_utils_enable_actions		(GtkActionGroup *action_group,
						 const GtkActionEntry *entries,
						 guint n_entries,
						 gboolean enable);
#endif

G_END_DECLS

#endif /* M_UTILS_H */
