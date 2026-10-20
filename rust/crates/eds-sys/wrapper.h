/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * The single translation unit for bindgen. The EDS umbrella headers do not
 * permit partial inclusion (they #error without the E_*_H_INSIDE guards).
 * Include each full library and let the allowlist in build.rs select what
 * goes to Rust.
 */

#include <libebackend/libebackend.h>
#include <libedata-book/libedata-book.h>
#include <libedata-cal/libedata-cal.h>
/* Camel, for the mail provider. The data-server headers above supply only the
 * Camel types that they mention. This include gives CamelProvider and the
 * store and transport classes to the allowlist. */
#include <camel/camel.h>
