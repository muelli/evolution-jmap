/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Single translation unit handed to bindgen. The EDS umbrella headers refuse
 * to be included piecemeal (they #error without the E_*_H_INSIDE guards), so
 * pull in the whole of each library and let the allowlist in build.rs decide
 * what actually reaches Rust.
 */

#include <libebackend/libebackend.h>
#include <libedata-book/libedata-book.h>
#include <libedata-cal/libedata-cal.h>
/* Camel, for M5's mail provider. Already reachable through the two data-server
 * headers above, but only as far as the types they mention; including it here
 * is what puts CamelProvider and the store/transport classes in front of the
 * allowlist. */
#include <camel/camel.h>
