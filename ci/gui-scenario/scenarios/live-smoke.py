# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: the --live target itself (item 182(a)), not a server-specific fix.

Every other scenario here runs only against jmap-mockd. This one is the
proof that ci/gui-scenario/run.sh --live, its loopback live_proxy.py relay
and the rebase it turns on reach a real server end to end: write a mail
account for the seeded user, answer the "Mail authentication request"
dialog with the seeded password, and find the seeded Inbox message.

Needs run.sh --live and a domain harness/stalwart/seed-manual-test.py has
already seeded: GUI_SCENARIO_USER/_PASSWORD name that account,
GUI_SCENARIO_STATE_FILE names the --state file seed-manual-test.py wrote.
Not one of item 182(b)'s named scenarios: those convert an existing
mock-only scenario so it runs both ways; this one exists only to prove the
groundwork (a) builds, with no mock equivalent of its own.
"""

from scenario import (
    LIVE_PASSWORD,
    LIVE_USER,
    ScenarioFailure,
    authenticate,
    expect_no_errors,
    live_state,
    main,
    say,
    screenshot,
    select_folder,
    select_message,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail (live)"
# One of harness/stalwart/seed-manual-test.py's own seeded Inbox subjects.
SUBJECT = "Offline test 1: move me to Projects"


def setup():
    if not LIVE_USER:
        raise ScenarioFailure("no GUI_SCENARIO_USER: this scenario needs run.sh --live")
    write_standalone_mail_account(ACCOUNT, user=LIVE_USER)
    say(f"setup: account sources written for {LIVE_USER}")


def run():
    state = live_state()
    say(f"live domain: {state['domain']}")
    authenticate(LIVE_PASSWORD, remember=True, label="authentication-request")
    select_folder(ACCOUNT, "Inbox")
    select_message(SUBJECT)
    screenshot("connected-to-the-live-server")
    expect_no_errors()


main(setup, run)
