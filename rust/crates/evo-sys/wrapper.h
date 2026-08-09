/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Single translation unit handed to bindgen. Unlike the EDS umbrella headers,
 * Evolution's own install their headers unguarded, so this includes exactly the
 * one the setup module is written against and lets it pull in what it needs
 * (gtk, camel, libebackend and e-util).
 */

#include <mail/e-mail-config-service-backend.h>
