# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: an edit of a event in its open editor and a server edit of a different field.

See edit_conflict.py for the steps and the checks.
"""

from edit_conflict import EVENT, run, setup
from scenario import main

main(lambda: setup(EVENT), lambda: run(EVENT), component="calendar")
