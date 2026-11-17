# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: the Vacation Responder, Server Filters and Delegates pages of
the account editor while offline (ROADMAP item 176(a), the "Server
Filters/Vacation offline" row of the offline matrix, and item 174's
read-only offline states).

Before this scenario's fix, opening either page while the shell is offline
(File > Work Offline, e_shell_get_online()) made the page try to connect
anyway: it sat on "Contacting the mail server..." for the whole of
jmap-client's 30-second connect timeout, then showed a generic
transport-error sentence built around that failure. Both pages now check
the shell's online state before their own network call, so they report
"you are offline" well inside that timeout instead.

The short `wait_for` timeout below is deliberate: it is well under the
30-second connect timeout, so a reverted fix (the page falling through to
its old "wait, then fail" behaviour) makes this scenario time out and fail,
not pass late.

Item 174's audit of each page's read-only offline state runs here too: a
page's offline note and its now-insensitive controls still need an
accessible name each, and still need unique mnemonics, the same rule the
page's online state already meets. Added to this scenario, not a new one,
because it needs the same "already offline, page open" setup.
"""

from scenario import (
    Jmap,
    ScenarioFailure,
    audit_mnemonics,
    do_action,
    evolution,
    expect_no_errors,
    find,
    folder_tree,
    main,
    menu_item,
    pyatspi,
    say,
    screenshot,
    select_folder,
    unnamed_widgets,
    wait_for,
    window,
    write_collection_mail_account,
)

ACCOUNT = "JMAP scenario mail"
EDITOR = "Account Editor"
VACATION_PAGE = "Vacation Responder"
FILTERS_PAGE = "Server Filters"
DELEGATES_PAGE = "Delegates"
OFFLINE_VACATION = (
    "You are offline. The autoresponder cannot be read or changed until you reconnect."
)
OFFLINE_FILTERS = "You are offline. Filters cannot be read or changed until you reconnect."
OFFLINE_DELEGATES = "You are offline. Delegates cannot be read until you reconnect."
# Well under jmap-client's 30-second connect timeout: see the module note.
OFFLINE_MESSAGE_TIMEOUT = 10
# The X keysym of the Menu key.
MENU_KEY = 0xFF67


def setup():
    write_collection_mail_account(ACCOUNT)
    say("setup: account source written")


def open_page(page_name):
    """Open the account editor of ACCOUNT on `page_name` and return the editor."""
    select_folder(ACCOUNT, "Inbox")
    cell = wait_for(
        lambda: find(folder_tree(), "table cell", ACCOUNT), f"{ACCOUNT!r} in the folder tree"
    )
    cell.queryComponent().grabFocus()
    pyatspi.Registry.generateKeyboardEvent(MENU_KEY, None, pyatspi.KEY_SYM)
    entry = wait_for(
        lambda: find(evolution(), "menu item", "Properties", showing=True),
        "the Properties item of the account menu",
    )
    do_action(entry)
    editor = wait_for(lambda: window(EDITOR), "the account editor")
    entry = wait_for(
        lambda: find(editor, "radio button", page_name), f"the {page_name!r} sidebar entry"
    )
    do_action(entry)
    say(f"opened the {page_name!r} page")
    return editor


def audit_offline_a11y(editor, page_name):
    """Raise if the account editor, with `page_name` open and read-only
    offline, has an unnamed focusable widget or a duplicate mnemonic
    (item 174's read-only offline state check).

    Checks the whole editor, not just the page: the editor's own sidebar
    and OK/Cancel/Help row stay sensitive and in scope, the same way the
    online a11y audit (a11y-audit.py) checks them for every page.
    """
    offenders = list(unnamed_widgets(editor))
    if offenders:
        raise ScenarioFailure(
            f"unnamed focusable widgets offline on the {page_name!r} page: {offenders}"
        )
    say(f"a11y-audit: no unnamed focusable widget offline on the {page_name!r} page")
    audit_mnemonics(editor, page_name)


def close_with_cancel(editor):
    cancel = wait_for(
        lambda: find(editor, "push button", "Cancel", showing=True), "the editor's Cancel button"
    )
    do_action(cancel)
    wait_for(lambda: window(EDITOR) is None, "the account editor closes")


def run():
    Jmap()

    # The account must sync once online first: the shell toggle below only
    # makes it stop using a connection it has, it does not make a never-yet
    # connected account appear in the folder tree.
    select_folder(ACCOUNT, "Inbox")
    say(f"online: {ACCOUNT!r} is synced")

    menu_item("File/Work Offline")
    say("menu: File > Work Offline")

    editor = open_page(VACATION_PAGE)
    wait_for(
        lambda: find(editor, "status bar", OFFLINE_VACATION),
        "the offline message on the Vacation Responder page",
        timeout=OFFLINE_MESSAGE_TIMEOUT,
    )
    say(f"Vacation Responder: {OFFLINE_VACATION!r}")
    screenshot("vacation-offline")
    audit_offline_a11y(editor, VACATION_PAGE)
    close_with_cancel(editor)

    editor = open_page(FILTERS_PAGE)
    wait_for(
        lambda: find(editor, "status bar", OFFLINE_FILTERS),
        "the offline message on the Server Filters page",
        timeout=OFFLINE_MESSAGE_TIMEOUT,
    )
    say(f"Server Filters: {OFFLINE_FILTERS!r}")
    screenshot("filters-offline")
    audit_offline_a11y(editor, FILTERS_PAGE)
    close_with_cancel(editor)

    editor = open_page(DELEGATES_PAGE)
    wait_for(
        lambda: find(editor, "status bar", OFFLINE_DELEGATES),
        "the offline message on the Delegates page",
        timeout=OFFLINE_MESSAGE_TIMEOUT,
    )
    say(f"Delegates: {OFFLINE_DELEGATES!r}")
    screenshot("delegates-offline")
    audit_offline_a11y(editor, DELEGATES_PAGE)
    close_with_cancel(editor)

    menu_item("File/Work Online")
    say("menu: File > Work Online")

    expect_no_errors()


main(setup, run)
