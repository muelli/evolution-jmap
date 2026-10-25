# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: the delete of a shared address book, with its undo window.

See shared_delete.py for the steps and the checks.
"""

from scenario import main
from shared_delete import ADDRESS_BOOK, MOCK_ARGS, run, setup

main(lambda: setup(ADDRESS_BOOK), lambda: run(ADDRESS_BOOK), component="addressbook", mock_args=MOCK_ARGS)
