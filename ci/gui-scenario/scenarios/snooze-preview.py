# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: Snooze in the preview pane of a detached message window.

`snooze/action.rs` merges the Snooze submenu into `/mail-message-popup`, the
context menu of the message list. `EMailBrowser` ("Open in New Window") shows
`mail-preview-popup` instead when the user right-clicks the message body, so
Snooze was unreachable there.

1. Open a message with "Open in New Window".
2. Right-click the message body. The context menu shows exactly one Snooze
   submenu, sensitive (the mock offers the Cyrus mail extension).
3. Snooze > For One Hour. The server moves the message to the snoozed
   mailbox.
"""

from scenario import (
    Jmap,
    ScenarioFailure,
    descendants,
    do_action,
    evolution,
    find,
    is_showing,
    main,
    menu_item,
    pyatspi,
    right_click,
    say,
    screenshot,
    select_folder,
    select_message,
    wait_for,
    window,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
SUBJECT = "Welcome to jmap-mockd"
CYRUS_MAIL = "https://cyrusimap.org/ns/jmap/mail"
ESCAPE = 0xFF1B


def setup():
    write_standalone_mail_account(ACCOUNT)
    say("setup: account sources written")


def snooze_submenus():
    """Every showing Snooze submenu in Evolution's context menus."""
    return [node for node in descendants(evolution(), "menu") if node.name == "Snooze" and is_showing(node)]


def close_popup():
    """Close the context menu with Escape if it is still open."""
    if find(evolution(), "menu", "Snooze", showing=True) is not None:
        pyatspi.Registry.generateKeyboardEvent(ESCAPE, None, pyatspi.KEY_SYM)
        say("closed the context menu")


def run():
    server = Jmap()
    select_folder(ACCOUNT, "Inbox")
    select_message(SUBJECT)
    menu_item("Message/Open in New Window")
    browser = wait_for(lambda: window(SUBJECT), "the detached message window")
    screenshot("detached-window")

    body = wait_for(
        lambda: find(browser, "document web", name="Evolution Mail Display"),
        "the message body",
    )
    right_click(body)
    say("right-clicked the message body")

    found = wait_for(lambda: snooze_submenus() or None, "the Snooze submenu")
    if len(found) != 1:
        screenshot("snooze-submenu-count")
        raise ScenarioFailure(f"the preview context menu has {len(found)} Snooze submenus, not one")
    submenu = found[0]
    wait_for(
        lambda: submenu.getState().contains(pyatspi.STATE_SENSITIVE),
        "the Snooze submenu sensitive (the gate found the extension)",
    )
    screenshot("preview-context-menu")
    entry = wait_for(lambda: find(submenu, "menu item", "For One Hour"), "the item 'For One Hour'")
    do_action(entry)
    say("menu Snooze/For One Hour")
    close_popup()

    snoozed = wait_for(lambda: server.mailbox_id_or_none("snoozed"), "a snoozed mailbox")

    def moved():
        email_id = server.emails()[SUBJECT]["id"]
        found = server.call(
            "Email/get",
            {"ids": [email_id], "properties": ["mailboxIds", "snoozed"]},
            extra_using=(CYRUS_MAIL,),
        )["list"][0]
        mailboxes = set(found["mailboxIds"])
        return found.get("snoozed") if mailboxes == {snoozed} else None

    details = wait_for(moved, f"{SUBJECT!r} only in the snoozed mailbox, with a wake time")
    say(f"server: {SUBJECT!r} snoozed until {details['until']}")
    screenshot("snoozed-from-preview")


main(setup, run, mock_args=("--snooze-extension",))
