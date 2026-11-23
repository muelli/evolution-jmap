# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: a queued move, copy or delete shows at once, offline (ROADMAP
item 181(a), 182(b)'s "offline-queue-visible" row).

The manual test of master `f3a62f4f` (F6) found that moving or copying a
message while offline left no visible mark: the row stayed in the source
folder, with no way to tell a change was queued. `jmap-mail` now hides a
queued move's source row (`75b3e80a8`) and shows a queued move or copy in
its destination folder as soon as that folder is next opened (`e83127a2`),
the Camel-layer half already covered by `jmap-mail/tests/transfer.rs`. This
scenario drives the same change through the real Evolution UI: File >
Work Offline, then Message > Move to Folder.../Copy to Folder... on two
messages, checking the message list of each folder before reconnecting.

With `--live` (item 181's live verification), the account is the seeded
live user's own, and the messages and the `Projects`/`Receipts` mailboxes
are `harness/stalwart/seed-manual-test.py`'s own fixtures, seeded there
for this exact check, instead of this scenario's own setup step creating
them against the mock. The live run also deletes a third seeded message
while offline: `jmap-mail/tests/expunge.rs` already covers the Camel-layer
half (a row marked deleted while disconnected stays in the folder, and the
server write happens only after reconnect), and this checks the same path
through the real UI and a real server's trash mailbox.

With `--live`, this scenario also breaks the connection at replay time, for
the move row (item 181's open "connection broken at replay time" check).
The account goes through its own `live_proxy.py`, on a port apart from
`run.sh --live`'s own proxy. This scenario stops that proxy right before
clicking Work Online, a full outage, not one failed request:
`jmap_mail::service` retries its own reconnect and fails to authenticate,
so the retry loop never reaches a session to send the filing's replay
call on. The queued move stays unsent, the same as while offline. This
scenario then restarts the proxy and checks the move still lands, once
the retry loop's own next attempt succeeds. `offline-replay-transport-
error.py` checks a narrower failure already, one reset request mid-replay
through `fault_proxy.py`, with a session already open.
"""

import os
import signal
import subprocess
import sys
import urllib.parse

from scenario import (
    ARTIFACTS,
    JMAP_URL,
    LIVE_PASSWORD,
    LIVE_USER,
    MOCK_PORT,
    Jmap,
    ScenarioFailure,
    authenticate,
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
MOVE_SUBJECT = "Welcome to jmap-mockd"
COPY_SUBJECT = "Second message"
# harness/stalwart/seed-manual-test.py's own fixtures for this check.
LIVE_MOVE_SUBJECT = "Offline test 1: move me to Projects"
LIVE_COPY_SUBJECT = "Offline test 2: copy me to Receipts"
LIVE_DELETE_SUBJECT = "Offline test 3: delete me"
PROJECTS = "Projects"
RECEIPTS = "Receipts"
# A second live proxy, apart from `run.sh --live`'s own on `MOCK_PORT`, so
# this scenario can stop and restart the connection on its own.
PROXY_PORT = int(MOCK_PORT) + 1
PID_FILE = os.path.join(ARTIFACTS, "live-proxy.pid")


def _live_target():
    """Return the real server's (host, port) named by `GUI_SCENARIO_JMAP_URL`."""
    parsed = urllib.parse.urlsplit(JMAP_URL)
    return parsed.hostname, parsed.port or (443 if parsed.scheme == "https" else 80)


def start_live_proxy():
    """Start a `live_proxy.py` on `PROXY_PORT`, in front of the real server.

    Its pid goes to `PID_FILE`; `stop_live_proxy` reads it back. Called
    twice in a live run: once in "setup", once more to restore the
    connection after "run" stops it.
    """
    host, port = _live_target()
    log_path = os.path.join(ARTIFACTS, "live-proxy.log")
    with open(log_path, "a", encoding="utf-8") as log:
        proc = subprocess.Popen(
            [sys.executable, "-m", "live_proxy", str(PROXY_PORT), host, str(port)],
            stdout=log,
            stderr=log,
        )
    with open(PID_FILE, "w", encoding="utf-8") as pid_file:
        pid_file.write(str(proc.pid))


def stop_live_proxy():
    """Stop the `live_proxy.py` named by `PID_FILE`, a real connection break."""
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
    if LIVE_USER:
        start_live_proxy()
        write_standalone_mail_account(ACCOUNT, port=PROXY_PORT, user=LIVE_USER)
        say("setup: live account source written, seeded fixtures reused")
    else:
        write_standalone_mail_account(ACCOUNT)
        server = Jmap()
        server.create_mailbox(PROJECTS)
        server.create_mailbox(RECEIPTS)
        say(f"setup: account with the {PROJECTS!r} and {RECEIPTS!r} mailboxes")


def message_shown(subject):
    """Whether `subject` is a showing row of the folder now open."""
    return find(message_list(), "table cell", subject, showing=True) is not None


def gone_from(subject, folder):
    wait_for(lambda: not message_shown(subject), f"{subject!r} gone from {folder!r}")
    say(f"Evolution: {subject!r} left {folder!r}")


def shown_in(subject, folder):
    wait_for(lambda: message_shown(subject), f"{subject!r} shown in {folder!r}")
    say(f"Evolution: {subject!r} shown in {folder!r}")


def transfer_to_folder(subject, folder, *, move):
    """Choose Message > Move/Copy to Folder... on `subject`, to `folder`.

    The dialog's own folder tree shares its accessible name ("Mail Folder
    Tree") with the main window's; every lookup below is rooted at the
    dialog, never at the whole application, to tell the two apart.
    """
    verb = "Move" if move else "Copy"
    select_message(subject)
    menu_item(f"Message/{verb} to Folder…")
    dialog = wait_for(lambda: window(f"{verb} to Folder"), f"the {verb!r} to Folder dialog")
    tree = wait_find("tree table", "Mail Folder Tree", root=dialog)
    cell = wait_for(lambda: find(tree, "table cell", folder), f"folder {folder!r} in the dialog")
    cell.queryComponent().grabFocus()
    screenshot(f"{verb.lower()}-dialog-{folder.lower()}")
    button = wait_find("push button", verb, root=dialog)
    wait_for(lambda: button.getState().contains(pyatspi.STATE_SENSITIVE), f"{verb!r} sensitive")
    do_action(button)
    wait_for(lambda: window(f"{verb} to Folder") is None, f"the {verb!r} to Folder dialog closes")
    say(f"menu Message/{verb} to Folder: {subject!r} to {folder!r}")


def run():
    try:
        _run()
    finally:
        if LIVE_USER:
            stop_live_proxy()


def _run():
    move_subject = LIVE_MOVE_SUBJECT if LIVE_USER else MOVE_SUBJECT
    copy_subject = LIVE_COPY_SUBJECT if LIVE_USER else COPY_SUBJECT
    if LIVE_USER:
        authenticate(LIVE_PASSWORD, remember=True, label="authentication-request")
        server = Jmap(user=LIVE_USER, password=LIVE_PASSWORD, rebase=True)
    else:
        server = Jmap()
    mailboxes = server.mailboxes()
    inbox_id = mailboxes["Inbox"]["id"]
    projects_id = mailboxes[PROJECTS]["id"]
    receipts_id = mailboxes[RECEIPTS]["id"]
    if LIVE_USER:
        trash_id = server.mailbox_id("trash")
        trash_name = next(name for name, box in mailboxes.items() if box["id"] == trash_id)

    # Open every folder once online, so Evolution already has a local
    # summary for the two new ones before going offline: the "no downsync
    # for a folder never opened before" gap (item 176) is a separate
    # question from the one this scenario checks.
    select_folder(ACCOUNT, "Inbox")
    select_folder(ACCOUNT, PROJECTS)
    select_folder(ACCOUNT, RECEIPTS)
    if LIVE_USER:
        select_folder(ACCOUNT, trash_name)
    select_folder(ACCOUNT, "Inbox")
    screenshot("01-online")

    menu_item("File/Work Offline")
    say("menu: File > Work Offline")

    transfer_to_folder(move_subject, PROJECTS, move=True)
    gone_from(move_subject, "Inbox")
    transfer_to_folder(copy_subject, RECEIPTS, move=False)
    shown_in(copy_subject, "Inbox")
    screenshot("02-inbox-after-queue")

    select_folder(ACCOUNT, PROJECTS)
    shown_in(move_subject, PROJECTS)
    screenshot("03-projects-queued-move")

    select_folder(ACCOUNT, RECEIPTS)
    shown_in(copy_subject, RECEIPTS)
    screenshot("04-receipts-queued-copy")

    if server.mailbox_of(move_subject) != {inbox_id}:
        raise ScenarioFailure(f"{move_subject!r} reached the server while offline")
    if server.mailbox_of(copy_subject) != {inbox_id}:
        raise ScenarioFailure(f"{copy_subject!r} reached the server while offline")
    say("server: neither queued filing was sent while offline")
    if log_count(r"queued filing") < 2:
        raise ScenarioFailure("the log does not show both filings queued")

    if LIVE_USER:
        select_folder(ACCOUNT, "Inbox")
        select_message(LIVE_DELETE_SUBJECT)
        menu_item("Edit/Delete Message")
        say("menu: Edit > Delete Message")
        screenshot("04b-inbox-after-delete-offline")
        if server.mailbox_of(LIVE_DELETE_SUBJECT) != {inbox_id}:
            raise ScenarioFailure(f"{LIVE_DELETE_SUBJECT!r} reached the server while offline")
        say("server: the delete was not sent while offline")

    if LIVE_USER:
        stop_live_proxy()
        say("stopped the live proxy: the connection breaks now")

    menu_item("File/Work Online")
    say("menu: File > Work Online")

    if LIVE_USER:
        # A full outage fails at reconnect, before a session exists to send
        # the filing's replay call on: `jmap_mail::service` retries the
        # authentication itself, never reaching `replay_filing`.
        expect_log(
            r"mail service authentication failed",
            "the broken connection fails to reconnect",
        )
        if server.mailbox_of(move_subject) != {inbox_id}:
            raise ScenarioFailure(
                f"{move_subject!r} reached the server despite the broken connection"
            )
        say("server: the move did not land while the connection was broken")
        screenshot("04c-kept-pending-broken-connection")
        start_live_proxy()
        say("restarted the live proxy: the connection is back")
        # The failed attempts above do not retry on their own within a
        # normal wait: the toggle itself is what starts a reconnect, proven
        # by every "File/Work Online" click so far, so repeat it to prompt
        # one more against the now-restored connection.
        menu_item("File/Work Offline")
        menu_item("File/Work Online")
        say("menu: File > Work Offline, then Work Online again")

    # A broken connection backs off to a slow retry (about 60s seen here),
    # once its first fast burst of attempts fails: a longer timeout than
    # the default only matters for the `LIVE_USER` connection-break check
    # above; an unbroken mock or live connection meets this long before.
    landing_timeout = 150 if LIVE_USER else 60
    wait_for(
        lambda: server.mailbox_of(move_subject) == {projects_id},
        f"{move_subject!r} only in {PROJECTS!r}",
        timeout=landing_timeout,
    )
    wait_for(
        lambda: server.mailbox_of(copy_subject) == {inbox_id, receipts_id},
        f"{copy_subject!r} in both {RECEIPTS!r} and Inbox",
        timeout=landing_timeout,
    )
    say("server: the move landed alone, the copy landed alongside the original")
    expect_log(r"replayed filing", "a queued filing replayed on reconnect")
    screenshot("05-replayed-online")

    if LIVE_USER:
        wait_for(
            lambda: server.mailbox_of(LIVE_DELETE_SUBJECT) == {trash_id},
            f"{LIVE_DELETE_SUBJECT!r} moved to {trash_name!r}",
        )
        say("server: the offline delete reached the trash on reconnect")
        expect_log(r"trashing message", "the deferred trash move")

    select_folder(ACCOUNT, "Inbox")
    gone_from(move_subject, "Inbox")
    shown_in(copy_subject, "Inbox")
    if LIVE_USER:
        gone_from(LIVE_DELETE_SUBJECT, "Inbox")

    expect_no_errors()


main(setup, run)
