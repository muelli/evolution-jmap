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
