# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: the delete of a shared calendar, with its undo window.

See shared_delete.py for the steps and the checks.
"""

from scenario import main
from shared_delete import CALENDAR, MOCK_ARGS, run, setup

main(lambda: setup(CALENDAR), lambda: run(CALENDAR), component="calendar", mock_args=MOCK_ARGS)
