# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: a queued offline filing survives a transport failure on
reconnect, not only a plain disconnect (ROADMAP item 181(b), 182(b)'s
"offline-replay-transport-error" row).

The manual test of master `f3a62f4f` (F7) found that after a failed
reconnect and a restart, a queued move or copy could land or could be lost
depending on exactly how the reconnect failed: `jmap-mail`'s
`replay_filing` only restored a queued filing's retry mark for
`StoreError::Disconnected`; a transport error (`Err(other)`, a connection
that resets mid-call rather than one that never starts) could drop the
mark and strand the filing (fixed, `jmap-mail/tests/transfer.rs`'s
`a_transport_failure_traces_the_kept_filing`; traced in `transfer.rs`'s
`replay_filing`, "kept filing pending after a transport failure").

A killed jmap-mockd cannot reach that code path through the real GUI: the
reconnect itself fails at the service layer first, so `replay_filing`
only ever sees `Disconnected`
(`notes/OFFLINE-REPLAY-TRANSPORT-ERROR-GUI-GAP-2026-11-15.md`). This
scenario routes the account through `fault_proxy.py` instead: a connect
and the session fetch succeed normally, and only the first request that
patches `mailboxIds` (a filing; never a plain keyword update) resets the
connection after that, giving the transport-failure arm a real,
GUI-driven trigger. The trigger is a filing specifically, not any
`Email/set`, because selecting the message to move it also queues a
`$seen` keyword update (no `mark-seen` timer needed: Camel marks a
previewed row read on its own); that update replays in the same
reconnect batch, ahead of the filing, and would otherwise spend the
one-shot trigger on the wrong call. One hit only: `synchronize_sync`
itself runs more than once per reconnect (Camel's own repeated walk of
the work list, not a scenario action), so the retry that sends the exact
same filing a moment later already goes through the proxy unchanged and
lands, with no second offline/online cycle needed.
"""

import os
import signal
import subprocess
import sys

from scenario import (
    ARTIFACTS,
    MOCK_PORT,
    Jmap,
    ScenarioFailure,
    do_action,
    expect_log,
    expect_no_errors,
    find,
    log_count,
    main,
    menu_item,
    message_list,
    pyatspi,
    say,
    screenshot,
    select_folder,
    select_message,
    wait_find,
    wait_for,
    window,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
SUBJECT = "Welcome to jmap-mockd"
PROJECTS = "Projects"
PROXY_PORT = int(MOCK_PORT) + 1
PID_FILE = os.path.join(ARTIFACTS, "fault-proxy.pid")


def message_shown(subject):
    return find(message_list(), "table cell", subject, showing=True) is not None


def move_to_folder(subject, folder):
    """Choose Message > Move to Folder... on `subject`, to `folder`."""
    select_message(subject)
    menu_item("Message/Move to Folder…")
    dialog = wait_for(lambda: window("Move to Folder"), "the Move to Folder dialog")
    tree = wait_find("tree table", "Mail Folder Tree", root=dialog)
    cell = wait_for(lambda: find(tree, "table cell", folder), f"folder {folder!r} in the dialog")
    cell.queryComponent().grabFocus()
    button = wait_find("push button", "Move", root=dialog)
    wait_for(lambda: button.getState().contains(pyatspi.STATE_SENSITIVE), "Move sensitive")
    do_action(button)
    wait_for(lambda: window("Move to Folder") is None, "the Move to Folder dialog closes")
    say(f"menu Message/Move to Folder: {subject!r} to {folder!r}")


def start_fault_proxy():
    """Start `fault_proxy.py` detached, in front of the mock, resetting the
    connection the first time it sees a filing (a request that patches
    `mailboxIds`). Its pid outlives this "setup" process; "run" stops it
    by `PID_FILE`.
    """
    log_path = os.path.join(ARTIFACTS, "fault-proxy.log")
    with open(log_path, "w", encoding="utf-8") as log:
        proc = subprocess.Popen(
            [sys.executable, "-m", "fault_proxy", str(PROXY_PORT), str(MOCK_PORT), "mailboxIds"],
            stdout=log,
            stderr=log,
        )
    with open(PID_FILE, "w", encoding="utf-8") as pid_file:
        pid_file.write(str(proc.pid))


def stop_fault_proxy():
    try:
        with open(PID_FILE, encoding="utf-8") as pid_file:
            pid = int(pid_file.read().strip())
    except (FileNotFoundError, ValueError):
        return
    try:
        os.kill(pid, signal.SIGTERM)
    except OSError:
        pass


def setup():
    start_fault_proxy()
    write_standalone_mail_account(ACCOUNT, port=PROXY_PORT)
    server = Jmap()
    server.create_mailbox(PROJECTS)
    say(f"setup: account through the fault proxy, {PROJECTS!r} mailbox created")


def run():
    try:
        server = Jmap()
        mailboxes = server.mailboxes()
        inbox_id = mailboxes["Inbox"]["id"]
        projects_id = mailboxes[PROJECTS]["id"]

        select_folder(ACCOUNT, "Inbox")
        select_folder(ACCOUNT, PROJECTS)
        select_folder(ACCOUNT, "Inbox")
        screenshot("01-online")

        menu_item("File/Work Offline")
        say("menu: File > Work Offline")

        move_to_folder(SUBJECT, PROJECTS)
        wait_for(lambda: not message_shown(SUBJECT), f"{SUBJECT!r} gone from Inbox")
        if server.mailbox_of(SUBJECT) != {inbox_id}:
            raise ScenarioFailure(f"{SUBJECT!r} reached the server while offline")
        if log_count(r"queued filing") < 1:
            raise ScenarioFailure("the log does not show the filing queued")
        screenshot("02-queued-offline")

        menu_item("File/Work Online")
        say("menu: File > Work Online (the fault proxy breaks the first filing attempt)")

        expect_log(
            r"kept filing pending after a transport failure",
            "a transport failure keeps the filing pending, not dropped",
        )
        screenshot("03-kept-pending")

        wait_for(
            lambda: server.mailbox_of(SUBJECT) == {projects_id}, f"{SUBJECT!r} in {PROJECTS!r}"
        )
        say("server: the kept-pending filing replayed and landed")
        expect_log(r"replayed filing", "a queued filing replayed on reconnect")
        screenshot("04-replayed")

        select_folder(ACCOUNT, "Inbox")
        wait_for(lambda: not message_shown(SUBJECT), f"{SUBJECT!r} gone from Inbox")
        select_folder(ACCOUNT, PROJECTS)
        wait_for(lambda: message_shown(SUBJECT), f"{SUBJECT!r} shown in {PROJECTS!r}")

        expect_no_errors()
    finally:
        stop_fault_proxy()


main(setup, run)
