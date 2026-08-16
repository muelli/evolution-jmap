#!/bin/sh
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Writes po/evolution-jmap.pot from the sources po/POTFILES.in names.
#
# Run it after adding, removing, or editing a translatable string; the result
# is committed, and `jmap-backend-core/tests/potfiles.rs` holds it against the
# sources in both directions, so forgetting to run it is a red test rather than
# a string that silently never reaches a translator.
#
# The command lives here and not in a comment because two things have to agree
# on it — the catalogue in the tree and the person regenerating it — and a
# command nobody executes drifts from the one that produced the file.

set -eu

cd "$(dirname "$0")/.."

# Every function in `jmap-backend-core`'s `i18n` that takes a msgid is a
# keyword, `translate_with` included: its first argument is the message and its
# later ones are the values filled into the message's `%1$s` placeholders,
# which is the argument position `--keyword` defaults to.
#
# `-L C` because gettext 0.21 has no Rust parser. It reads our markers
# correctly — `N_(c"…")` is a call with a string argument in either language —
# but it is crude elsewhere, and the warnings it prints are worth reading. See
# the test named above for what that crudeness has been measured to cost.
xgettext \
    --files-from=po/POTFILES.in \
    -L C \
    --from-code=UTF-8 \
    --keyword=N_ \
    --keyword=translate \
    --keyword=translate_with \
    --add-comments=TRANSLATORS \
    --package-name=evolution-jmap \
    --copyright-holder='Tobias Mueller <muelli@cryptobitch.de>' \
    -o po/evolution-jmap.pot
