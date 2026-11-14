# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: the Delegates page's Add dialog refuses an empty or
no-access delegate (ROADMAP item 179(e), 182(b)'s "delegates-empty-add"
row).

Before this fix, OK closed the Add Delegate dialog on an empty principal
or on a principal with both levels left at "No access", granting
nothing. `jmap-ui`'s own in-process widget test
(`the_add_dialog_refuses_an_empty_or_no_access_delegate`) already proves
the GTK sensitivity logic in isolation; this scenario drives the same
guard through real AT-SPI and a real window, so any gap between the two
(an event a synthetic click or key does not actually trigger, for
example) would show up here and not just in the direct widget test.
"""

from scenario import (
    Jmap,
    ScenarioFailure,
    click_at,
    click_button,
    descendants,
    do_action,
    evolution,
    expect_no_errors,
    find,
    folder_tree,
    main,
    pyatspi,
    say,
    screenshot,
    select_folder,
    set_text,
    wait_for,
    window,
    write_collection_mail_account,
)

ACCOUNT = "JMAP scenario mail"
EDITOR = "Account Editor"
PAGE = "Delegates"
# The X keysym of the Menu key.
MENU_KEY = 0xFF67

ADD_DIALOG = "Add Delegate"
OK_LABEL = "Add…"
PRINCIPAL = "carol@example.com"
WHITESPACE_PRINCIPAL = "   "
# CALENDAR_LEVELS index 3 is CalendarDelegationLevel::Author (not "No access").
CALENDAR_LEVEL_ROW = 3


def setup():
    write_collection_mail_account(ACCOUNT)
    server = Jmap()
    calendar_id = server.create("Calendar", {"name": "Work"})
    book_id = server.create("AddressBook", {"name": "Contacts"})
    say(f"setup: account written, calendar {calendar_id} and book {book_id} created")


def open_page():
    """Open the account editor of ACCOUNT on the Delegates page."""
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
    entry = wait_for(lambda: find(editor, "radio button", PAGE), f"the {PAGE!r} sidebar entry")
    do_action(entry)
    say(f"opened the {PAGE!r} page")
    return editor


def loaded(editor):
    """Wait until the page has loaded and return the Add button."""
    add = wait_for(
        lambda: find(editor, "push button", "Add…", showing=True), "a showing Add button"
    )
    wait_for(lambda: add.getState().contains(pyatspi.STATE_SENSITIVE), "the page loaded")
    return add


def close_with_cancel(editor):
    cancel = wait_for(
        lambda: find(editor, "push button", "Cancel", showing=True), "the editor's Cancel button"
    )
    do_action(cancel)
    wait_for(lambda: window(EDITOR) is None, "the account editor closes")


def ok_button(dialog):
    return wait_for(lambda: find(dialog, None, OK_LABEL), f"the {OK_LABEL!r} button")


def is_sensitive(node):
    return node.getState().contains(pyatspi.STATE_SENSITIVE)


def shared_with(server, type_name, object_id, principal):
    found = server.get(type_name, object_id)
    if found is None:
        raise ScenarioFailure(f"the server has no {type_name} {object_id}")
    return (found.get("shareWith") or {}).get(principal)


def run():
    server = Jmap()
    calendars = server.names("Calendar")
    calendar_id = next(iter(calendars))

    editor = open_page()
    add = loaded(editor)
    click_at(add)
    dialog = wait_for(lambda: window(ADD_DIALOG), "the Add Delegate dialog")
    entry = wait_for(lambda: find(dialog, "text"), "the principal entry")
    combos = list(descendants(dialog, "combo box"))
    if len(combos) != 2:
        raise ScenarioFailure(f"the Add dialog has {len(combos)} combo boxes, not 2")
    calendar_combo = combos[0]
    ok = ok_button(dialog)

    wait_for(lambda: not is_sensitive(ok), f"{OK_LABEL!r} insensitive with an empty principal")
    say(f"dialog: {OK_LABEL!r} insensitive, empty principal, both levels No access")
    screenshot("empty-principal")

    set_text(entry, WHITESPACE_PRINCIPAL)
    wait_for(
        lambda: not is_sensitive(ok), f"{OK_LABEL!r} insensitive with a whitespace-only principal"
    )
    say(f"dialog: {OK_LABEL!r} stays insensitive, whitespace-only principal")

    set_text(entry, PRINCIPAL)
    wait_for(
        lambda: not is_sensitive(ok), f"{OK_LABEL!r} insensitive with no access granted"
    )
    say(f"dialog: {OK_LABEL!r} stays insensitive, a principal but no access granted")
    screenshot("principal-no-access")

    set_text(entry, "")
    calendar_combo.querySelection().selectChild(CALENDAR_LEVEL_ROW)
    wait_for(
        lambda: not is_sensitive(ok), f"{OK_LABEL!r} insensitive with access but no principal"
    )
    say(f"dialog: {OK_LABEL!r} stays insensitive, access granted but the principal cleared again")

    set_text(entry, PRINCIPAL)
    wait_for(
        lambda: is_sensitive(ok), f"{OK_LABEL!r} sensitive once both a principal and access are set"
    )
    say(f"dialog: {OK_LABEL!r} sensitive, {PRINCIPAL!r} with calendar access")
    screenshot("valid")

    click_button(dialog, OK_LABEL)
    wait_for(lambda: window(ADD_DIALOG) is None, "the Add Delegate dialog closes")
    wait_for(lambda: find(editor, "table cell", PRINCIPAL), f"{PRINCIPAL!r} listed")
    say(f"page: {PRINCIPAL!r} listed, the dialog only let a real grant through")

    calendar_rights = wait_for(
        lambda: shared_with(server, "Calendar", calendar_id, PRINCIPAL),
        f"{PRINCIPAL!r} in the calendar's shareWith",
    )
    if not calendar_rights.get("mayWriteOwn"):
        raise ScenarioFailure(f"the calendar share is not Author: {calendar_rights}")
    say("server: the calendar shares with the principal at the chosen level")

    close_with_cancel(editor)
    expect_no_errors()


main(setup, run)
