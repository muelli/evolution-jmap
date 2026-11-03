# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: Evolution started with no network reachable (ROADMAP item
176(a), the "Evolution started with no network" row of the offline matrix).

work-offline.py toggles File > Work Offline on an already-running,
already-connected shell: the store never loses its live connection object,
it just stops using it. This scenario is the other condition the matrix
lists separately: a process that never makes a single successful
connection in its own lifetime, the GUI-level counterpart of
jmap-functional's restart-offline tests (176(a)/176(b)), which cover the
same shape directly through libcamel with no shell involved.

ci/gui-scenario/run.sh always starts the one Evolution process it launches
against an already-running jmap-mockd with --force-online, so there is no
way to get a process that starts with the server unreachable except by
doing a second launch from inside this scenario. run() therefore: lets the
first process (run.sh's own) sync normally, so 176(b)'s on-disk caches get
written; kills that process and jmap-mockd itself, so nothing listens on
the mock's port at all; starts a second Evolution process at the same
profile; and checks the folder tree and the message list still show the
account from the on-disk cache, with no blocking error dialog and no
crate error logged, even though this second process never once reaches
the server.
"""

import socket
import subprocess

from scenario import (
    ARTIFACTS,
    Jmap,
    evolution,
    expect_no_errors,
    main,
    say,
    screenshot,
    select_folder,
    select_message,
    wait_for,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
SUBJECT = "Welcome to jmap-mockd"
MOCK_PORT = 8090


def setup():
    write_standalone_mail_account(ACCOUNT)
    say("setup: account sources written")


def _mock_reachable():
    try:
        with socket.create_connection(("127.0.0.1", MOCK_PORT), timeout=1):
            return True
    except OSError:
        return False


def run():
    server = Jmap()
    server.emails()[SUBJECT]  # fails loudly if the mock did not seed it

    select_folder(ACCOUNT, "Inbox")
    select_message(SUBJECT)
    screenshot("01-first-launch-online")
    say("first launch, online: folder tree and message list synced")

    subprocess.run(["pkill", "-x", "evolution"], check=False)
    wait_for(lambda: evolution() is None, "the first Evolution process to exit")
    say("killed the first Evolution process")

    subprocess.run(["pkill", "-x", "jmap-mockd"], check=False)
    wait_for(lambda: not _mock_reachable(), "jmap-mockd to stop accepting connections")
    say("killed jmap-mockd: nothing listens on its port now")

    log_path = f"{ARTIFACTS}/evolution.log"
    with open(log_path, "ab") as log:
        process = subprocess.Popen(
            ["evolution", "-c", "mail", "--force-online"], stdout=log, stderr=log
        )
    try:
        wait_for(evolution, "the second Evolution process to register with AT-SPI")
        select_folder(ACCOUNT, "Inbox")
        select_message(SUBJECT)
        screenshot("02-second-launch-no-network")
        say("second launch, no network at all: folder tree and message list still show the account")
        expect_no_errors()
    finally:
        process.terminate()
        process.wait(timeout=10)


main(setup, run)
