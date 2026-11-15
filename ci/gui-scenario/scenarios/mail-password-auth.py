# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: a mail account that needs a password, and the password store.

Every other scenario gives the mock no credentials: the account has no user
name, so Evolution never asks for a password. A real server always needs
one, so the path that asks for it, sends it and keeps it had no GUI
coverage at all.

This scenario starts the mock with `--basic`, gives the account a user
name, and answers the "Mail authentication request" dialog with the
password and "remember". Then it checks two things:

1. The account connects with that password: the folder tree and the message
   list show the seeded Inbox message.
2. A second Evolution process in the same profile connects without a new
   question. Evolution keeps the password in the session secret store
   (`org.freedesktop.secrets`), not in the account file, so this step fails
   when no secret store answers.

Step 2 is why ci/gui-scenario/run.sh unlocks a keyring for each scenario.
"""

import os
import socket

from scenario import (
    ARTIFACTS,
    MOCK_PORT,
    ScenarioFailure,
    authentication_request,
    authenticate,
    expect_no_errors,
    main,
    restart_evolution,
    say,
    screenshot,
    select_folder,
    select_message,
    wait_for,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
SUBJECT = "Welcome to jmap-mockd"
USER = "alice"
PASSWORD = "scenario-secret"


def _mock_reachable():
    try:
        with socket.create_connection(("127.0.0.1", int(MOCK_PORT)), timeout=1):
            return True
    except OSError:
        return False


def setup():
    write_standalone_mail_account(ACCOUNT, user=USER)
    # The other scenarios wait for the mock with a Jmap call. A mock with
    # --basic refuses that call, so wait for its port instead.
    wait_for(_mock_reachable, "jmap-mockd to accept connections", timeout=30)
    say(f"setup: account sources written, user {USER}")


def _mock_calls():
    """The number of JMAP calls that the mock answered, from its own log.

    A call that carries no password, or a wrong one, never reaches this
    count: the mock refuses it before it logs the request.
    """
    path = os.path.join(ARTIFACTS, "mock.log")
    with open(path, encoding="utf-8", errors="replace") as log:
        return sum(1 for line in log if line.startswith("--> POST /jmap"))


def run():
    authenticate(PASSWORD, remember=True, label="authentication-request")

    select_folder(ACCOUNT, "Inbox")
    select_message(SUBJECT)
    screenshot("connected-with-the-password")
    say("the account connected with the password from the dialog")

    calls = _mock_calls()
    restart_evolution()

    def asked_or_called():
        if authentication_request() is not None:
            return "asked"
        return _mock_calls() > calls

    if wait_for(asked_or_called, "a server call or a password question after the restart") == "asked":
        raise ScenarioFailure(
            "Evolution asks for the password again after a restart: "
            "the secret store did not keep it"
        )
    say("the second process called the server with the stored password, with no question")

    select_folder(ACCOUNT, "Inbox")
    select_message(SUBJECT)
    screenshot("after-a-restart")
    expect_no_errors()


main(setup, run, mock_args=("--basic", f"{USER}:{PASSWORD}"))
