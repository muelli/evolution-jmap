# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: a stock IMAP account against ci/dovecot-oracle.sh, item 175's
comparison leg, first slice.

Item 175 needs a plain IMAP account open in the same harness as a JMAP one,
so a later scenario can check that a behaviour (not auto-fetching a vCard
photo) is Evolution's own, not specific to this provider. This slice only
proves the wiring: a stock `imapx` account reaches the oracle
(`ci/dovecot-oracle.sh`) over the podman network `ci/gui-scenario/run.sh`
now joins, and shows a message the host delivered into its Maildir before
Evolution started. The actual vCard-photo comparison is its own later
scenario, built on this one.

`ci/gui-scenario/run.sh` delivers `../fixtures/imap-oracle-smoke.eml` on the
host before this scenario's container starts (the container has no podman
of its own to reach the oracle's sibling container); the oracle itself must
already be running (`ci/dovecot-oracle.sh start`).
"""

from scenario import (
    ScenarioFailure,
    main,
    select_folder,
    select_message,
    write_standalone_imap_account,
)

ACCOUNT = "IMAP scenario mail"
SUBJECT = "IMAP oracle smoke test"


def setup():
    write_standalone_imap_account(ACCOUNT)


def run():
    # `select_folder` dismisses the mail authentication request itself (it
    # waits through `dismiss_first_run_dialogs`, repeated: "a store that
    # connects again asks for the password again"). The oracle's
    # `static`/`nopassword=y` passdb accepts the empty password that submits,
    # since there is no keyring here to cache a real one.
    select_folder(ACCOUNT, "Inbox")
    try:
        select_message(SUBJECT)
    except ScenarioFailure as failure:
        raise ScenarioFailure(
            f"the message delivered into the oracle's Maildir never showed "
            f"over IMAP: {failure}"
        ) from failure


main(setup, run)
