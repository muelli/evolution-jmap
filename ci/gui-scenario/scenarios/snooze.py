# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: Snooze in the context menu of the message list.

1. Snooze > For One Hour on a message of the inbox. The server gets a
   mailbox with the "snoozed" role, the message moves into it, and its
   Email.snoozed wakes it one hour from now. The Snoozed folder appears in
   Evolution, the inbox stays selected, and the message leaves it.
2. Snooze > Until Next Monday Morning on the other message. The server
   wakes it on a Monday at 09:00 local time.

The mock offers the Cyrus mail extension (--snooze-extension). The account
has no snoozed mailbox at the start: the client creates it.
"""

import time

from scenario import (
    Jmap,
    ScenarioFailure,
    descendants,
    do_action,
    evolution,
    expect_log,
    expect_no_errors,
    find,
    folder_cell,
    is_showing,
    main,
    message_list,
    popup_menu,
    pyatspi,
    say,
    screenshot,
    select_folder,
    wait_for,
    window,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
FIRST = "Welcome to jmap-mockd"
SECOND = "Second message"
CYRUS_MAIL = "https://cyrusimap.org/ns/jmap/mail"
ONE_HOUR = 3600
# The client takes the time from the local clock when the item is activated.
SLACK = 60
WORKDAY_HOUR = 9
MONDAY = 0
ESCAPE = 0xFF1B


def setup():
    write_standalone_mail_account(ACCOUNT)
    say("setup: account sources written")


def snoozed_state(server, subject):
    """Return (mailboxIds, snoozed) of the message with `subject`."""
    email_id = server.emails()[subject]["id"]
    found = server.call(
        "Email/get",
        {"ids": [email_id], "properties": ["mailboxIds", "snoozed"]},
        extra_using=(CYRUS_MAIL,),
    )["list"][0]
    return set(found["mailboxIds"]), found.get("snoozed")


def utc_seconds(stamp):
    """Seconds since the epoch of an RFC 3339 UTC date like 2026-10-24T14:00:00Z."""
    return time.mktime(time.strptime(stamp, "%Y-%m-%dT%H:%M:%SZ")) - time.timezone


def snooze_submenus():
    """Every Snooze submenu that Evolution shows, in tree order.

    One mail window can hold more than one EMailReader. Each reader loads
    the extensions of the EMailReader interface, but all readers of a window
    share one menu layer. A merge for each reader puts the submenu in the
    context menu twice. This function counts them.
    """
    return [
        node
        for node in descendants(evolution(), "menu")
        if node.name == "Snooze" and is_showing(node)
    ]


def snooze(subject, item):
    """Choose Snooze > `item` in the context menu of `subject`."""
    popup_menu(subject)
    submenu = wait_for(lambda: find(evolution(), "menu", "Snooze"), "the Snooze submenu")
    wait_for(
        lambda: submenu.getState().contains(pyatspi.STATE_SENSITIVE),
        "the Snooze submenu sensitive (the gate found the extension)",
    )
    screenshot(f"context-menu-{subject.split()[0].lower()}")
    found = snooze_submenus()
    if len(found) != 1:
        screenshot("snooze-submenu-count")
        raise ScenarioFailure(f"the context menu has {len(found)} Snooze submenus, not one")
    say("Evolution: the context menu has exactly one Snooze submenu")
    entry = wait_for(lambda: find(submenu, "menu item", item), f"the item {item!r}")
    # The action starts a worker and returns, so no child process is needed.
    do_action(entry)
    say(f"menu Snooze/{item}")


def close_popup():
    """Close the context menu with Escape if it is still open."""
    if find(evolution(), "menu", "Snooze", showing=True) is not None:
        pyatspi.Registry.generateKeyboardEvent(ESCAPE, None, pyatspi.KEY_SYM)
        say("closed the context menu")


def wait_snoozed(server, subject):
    """Wait until `subject` is only in the snoozed mailbox and return its wake time."""
    snoozed = wait_for(lambda: server.mailbox_id_or_none("snoozed"), "a snoozed mailbox")

    def moved():
        mailboxes, details = snoozed_state(server, subject)
        return details if mailboxes == {snoozed} and details else None

    details = wait_for(moved, f"{subject!r} only in the snoozed mailbox, with a wake time")
    say(f"server: {subject!r} snoozed until {details['until']}")
    if details.get("moveToMailboxId"):
        raise ScenarioFailure(f"the client set a wake target: {details}")
    return utc_seconds(details["until"])


def stays_in_inbox():
    """Fail if the new Snoozed folder took the selection away from the inbox.

    A folder that appears on the server must not rebuild the folder tree of
    the account: a rebuild loses the selected folder.
    """
    wait_for(lambda: folder_cell(ACCOUNT, "Snoozed"), "the Snoozed folder in the folder tree")
    if window("Inbox") is None:
        screenshot("selection-lost")
        raise ScenarioFailure("the window left the inbox when the Snoozed folder appeared")
    say("Evolution: the inbox stays selected after the Snoozed folder appeared")


def gone_from_inbox(subject):
    wait_for(
        lambda: find(message_list(), "table cell", subject, showing=True) is None,
        f"{subject!r} gone from the inbox in Evolution",
    )
    say(f"Evolution: {subject!r} left the inbox")


def run():
    server = Jmap()
    if server.mailbox_id_or_none("snoozed") is not None:
        raise ScenarioFailure("the mock already has a snoozed mailbox")
    select_folder(ACCOUNT, "Inbox")
    screenshot("inbox-before-snooze")

    # 1. For one hour.
    before = time.time()
    snooze(FIRST, "For One Hour")
    wake = wait_snoozed(server, FIRST)
    close_popup()
    after = time.time()
    if not before + ONE_HOUR - SLACK <= wake <= after + ONE_HOUR + SLACK:
        raise ScenarioFailure(f"the wake time is {wake - before:.0f}s ahead, not one hour")
    say(f"server: wakes {wake - before:.0f}s after the click")
    expect_log(r"jmap_ui.*messages snoozed.*count=1", "the snooze of one message")
    stays_in_inbox()
    gone_from_inbox(FIRST)
    screenshot("inbox-after-first-snooze")

    # 2. Until next Monday morning.
    snooze(SECOND, "Until Next Monday Morning")
    wake = wait_snoozed(server, SECOND)
    close_popup()
    local = time.localtime(wake)
    if local.tm_wday != MONDAY or (local.tm_hour, local.tm_min) != (WORKDAY_HOUR, 0):
        raise ScenarioFailure(f"the wake time {time.asctime(local)} is not a Monday at 09:00")
    # jmap-ui's next_monday() always skips a full week on a Monday, even
    # before WORKDAY_HOUR. See schedule.rs's next_monday and its
    # the_next_monday_from_a_fixed_clock test. A run on a Monday before
    # 09:00 local time can land just over 7 days ahead.
    if not 0 < wake - time.time() <= (7 * 24 + WORKDAY_HOUR) * ONE_HOUR:
        raise ScenarioFailure(f"the wake time {time.asctime(local)} is not in the next 7 days")
    say(f"server: wakes {time.asctime(local)} local time")
    gone_from_inbox(SECOND)
    screenshot("inbox-after-second-snooze")
    expect_no_errors()


main(setup, run, mock_args=("--snooze-extension",))
