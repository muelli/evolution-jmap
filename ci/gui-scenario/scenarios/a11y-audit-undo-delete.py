# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: the undo-delete info bar has no unnamed focusable widget and
no duplicate Alt+letter mnemonic.

Item 174(d)'s automated audit (see a11y-audit.py) applied to the one
surface that needs a calendar component instead of a mail one: the info
bar `undo_delete/shell_ext.rs` submits after a collection delete. Reuses
`shared_delete.py`'s setup and the first delete step of its scenario
(shared-calendar-delete.py), stopping once the info bar shows: the rest
of that scenario's undo/wait/destroy steps are shared_delete.py's own
job, not this audit's.

Not run here: `audit_tab_reachability` (item 174(d)'s live Tab-key
check, see a11y-audit.py). Confirmed empirically that pressing Tab after
the info bar appears, right after a confirmation dialog closes, leaves
no STATE_FOCUSED node observable anywhere for the rest of the walk, the
same symptom a11y-audit.py's Send Later dialog hits and documents; root
cause not pinned down, logged as an open row rather than guessed at.
"""

from scenario import audit_dialog, audit_mnemonics, confirm, info_bar, main, menu_item, select_source, wait_for
from shared_delete import CALENDAR, MOCK_ARGS, SHARED_NAME, setup

DISPLAY = f"{SHARED_NAME}: {CALENDAR.collection}"
HEADLINE = f'{CALENDAR.label} "{DISPLAY}" is deleted.'


def run():
    select_source(DISPLAY)
    menu_item(CALENDAR.menu)
    confirm("Delete From Server", "before-info-bar")
    bar = wait_for(lambda: info_bar(HEADLINE), f"the info bar {HEADLINE!r}")
    audit_dialog(bar, "undo-delete")
    audit_mnemonics(bar, "undo-delete")


main(lambda: setup(CALENDAR), run, component="calendar", mock_args=MOCK_ARGS)
