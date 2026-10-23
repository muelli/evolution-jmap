/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Small helpers that many client programs use in the same form.
 */

#ifndef FUNCTIONAL_TEST_UTIL_H
#define FUNCTIONAL_TEST_UTIL_H

#include <libedataserver/libedataserver.h>

G_BEGIN_DECLS

int	functional_fail			(const gchar *step,
					 GError *error);
void	functional_observe		(const gchar *key,
					 const gchar *value);
void	functional_credentials_required_cb
					(ESource *source,
					 ESourceCredentialsReason reason,
					 const gchar *certificate_pem,
					 GTlsCertificateFlags certificate_errors,
					 const GError *op_error,
					 gpointer user_data);
void	functional_settle		(guint milliseconds);

G_END_DECLS

#endif /* FUNCTIONAL_TEST_UTIL_H */
