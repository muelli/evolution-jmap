/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Single translation unit handed to bindgen. Unlike the EDS umbrella headers,
 * Evolution's own install their headers unguarded, so this includes exactly the
 * one the setup module is written against and lets it pull in what it needs
 * (gtk, camel, libebackend and e-util).
 */

#include <mail/e-mail-config-service-backend.h>

/* And the page that class is an extension of, for the one accessor
 * `setup_defaults` needs off it — the address typed on the identity page. The
 * page itself stays an opaque handle (see build.rs); this is here so that the
 * accessor's declaration is. */
#include <mail/e-mail-config-service-page.h>
