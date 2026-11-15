# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: the Delegates page of the account editor (ROADMAP item 164).

1. The page loads with no delegate: the status bar says so.
2. Alice clicks Add, fills the principal id and the two level combos, and
   confirms. The server's calendar and address book both carry the chosen
   rights for that principal, and the page lists the new row.
3. Alice selects the row and clicks Revoke. The server no longer shares
   either collection with that principal, and the page shows the empty
   status bar again.

With `--live` (item 182(b)), the principal is `harness/stalwart/
seed-manual-test.py`'s own seeded "bob" account id, read from the --state
file: the Add dialog's field takes only a principal id today (ROADMAP item
185(b) is the open row for an address), and a real server, unlike the mock,
refuses an address that is not also a valid id.
"""

from scenario import (
    LIVE_PASSWORD,
    LIVE_USER,
    Jmap,
    ScenarioFailure,
    authenticate,
    click_at,
    click_button,
    descendants,
    do_action,
    evolution,
    expect_no_errors,
    find,
    folder_tree,
    live_state,
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

NO_DELEGATES = "No delegates yet."
PRINCIPAL = "bob@example.com"
ADD_DIALOG = "Add Delegate"
# In document order: [calendar level, address book level].
CALENDAR_LEVEL_ROW = 3  # CalendarDelegationLevel::Author
BOOK_LEVEL_ROW = 1  # AddressBookDelegationLevel::Reviewer
CALENDAR_LEVEL_NAME = "Author"
BOOK_LEVEL_NAME = "Reviewer"


def _server():
    if LIVE_USER:
        return Jmap(user=LIVE_USER, password=LIVE_PASSWORD, rebase=True)
    return Jmap()


def _principal_display(server, principal_id):
    """The table row text for `principal_id`, mirroring
    `jmap_ui::delegation::principal_label`: "name <email>", whichever of the
    two the server has, or the raw id with neither (the mock has no
    `Principal/get`, so a mock run always falls back to the raw id).
    """
    found = server.call(
        "Principal/get", {"ids": [principal_id]}, extra_using=("urn:ietf:params:jmap:principals",)
    )["list"]
    if not found:
        return principal_id
    name = (found[0].get("name") or "").strip()
    email = (found[0].get("email") or "").strip() or None
    if name and email:
        return f"{name} <{email}>"
    return name or email or principal_id


def setup():
    if LIVE_USER:
        write_collection_mail_account(ACCOUNT, user=LIVE_USER)
    else:
        write_collection_mail_account(ACCOUNT)
    server = _server()
    calendar_id = server.create("Calendar", {"name": "Work"})
    book_id = server.create("AddressBook", {"name": "Contacts"})
    if LIVE_USER:
        # `harness/stalwart/seed-manual-test.py seed` shares alice's default
        # calendar and address book with carol (item 164's manual-test
        # fixture), so the page would start with an existing delegate
        # instead of "No delegates yet." Clear every share so this
        # scenario's own Add/Revoke is the only delegate change in view.
        for existing_id in server.names("Calendar"):
            server.update("Calendar", existing_id, {"shareWith": {}})
        for existing_id in server.names("AddressBook"):
            server.update("AddressBook", existing_id, {"shareWith": {}})
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


def selected_item_name(combo):
    """Return the name of `combo`'s selected item, read from the selected child."""
    selection = combo.querySelection()
    if selection.nSelectedChildren < 1:
        return ""
    return selection.getSelectedChild(0).name


def shared_with(server, type_name, object_id, principal):
    found = server.get(type_name, object_id)
    if found is None:
        raise ScenarioFailure(f"the server has no {type_name} {object_id}")
    return (found.get("shareWith") or {}).get(principal)


def run():
    principal = PRINCIPAL
    if LIVE_USER:
        authenticate(LIVE_PASSWORD, remember=True, label="authentication-request")
        principal = live_state()["accounts"]["bob"]
    server = _server()
    # The mock has no Principal/get, so a mock run's row shows the raw
    # PRINCIPAL unchanged; a real server resolves it to a name/email label.
    principal_display = _principal_display(server, principal) if LIVE_USER else principal
    calendars = server.names("Calendar")
    books = server.names("AddressBook")
    calendar_id = next(iter(calendars))
    book_id = next(iter(books))

    # 1. The page loads with no delegate.
    editor = open_page()
    loaded(editor)
    wait_for(lambda: find(editor, "status bar", NO_DELEGATES), "the empty status bar")
    say(f"page: {NO_DELEGATES!r}")
    screenshot("delegates-loaded")

    # 2. Add a delegate with distinct, non-default levels.
    add = loaded(editor)
    click_at(add)
    dialog = wait_for(lambda: window(ADD_DIALOG), "the Add Delegate dialog")
    entry = wait_for(lambda: find(dialog, "text"), "the principal entry")
    set_text(entry, principal)
    combos = list(descendants(dialog, "combo box"))
    if len(combos) != 2:
        raise ScenarioFailure(f"the Add dialog has {len(combos)} combo boxes, not 2")
    combos[0].querySelection().selectChild(CALENDAR_LEVEL_ROW)
    wait_for(
        lambda: selected_item_name(combos[0]) == CALENDAR_LEVEL_NAME,
        f"the calendar level combo shows {CALENDAR_LEVEL_NAME!r}",
    )
    combos[1].querySelection().selectChild(BOOK_LEVEL_ROW)
    wait_for(
        lambda: selected_item_name(combos[1]) == BOOK_LEVEL_NAME,
        f"the address book level combo shows {BOOK_LEVEL_NAME!r}",
    )
    screenshot("delegates-add-filled")
    click_button(dialog, "Add…")
    wait_for(lambda: window(ADD_DIALOG) is None, "the Add Delegate dialog closes")
    wait_for(lambda: find(editor, "table cell", principal_display), f"{principal_display!r} listed")
    wait_for(lambda: find(editor, "table cell", CALENDAR_LEVEL_NAME), "its calendar level listed")
    wait_for(lambda: find(editor, "table cell", BOOK_LEVEL_NAME), "its address book level listed")
    say(f"page: {principal_display!r} listed with {CALENDAR_LEVEL_NAME!r}/{BOOK_LEVEL_NAME!r}")
    screenshot("delegates-added")

    calendar_rights = wait_for(
        lambda: shared_with(server, "Calendar", calendar_id, principal),
        f"{principal!r} in the calendar's shareWith",
    )
    if not calendar_rights.get("mayWriteOwn"):
        raise ScenarioFailure(f"the calendar share is not Author: {calendar_rights}")
    book_rights = wait_for(
        lambda: shared_with(server, "AddressBook", book_id, principal),
        f"{principal!r} in the address book's shareWith",
    )
    if not book_rights.get("mayRead") or book_rights.get("mayWrite"):
        raise ScenarioFailure(f"the address book share is not Reviewer: {book_rights}")
    say("server: both collections share with the principal at the chosen levels")

    # 3. Revoke clears both shares.
    row = wait_for(lambda: find(editor, "table cell", principal_display), f"{principal_display!r} listed")
    row.queryComponent().grabFocus()
    revoke = wait_for(
        lambda: find(editor, "push button", "Revoke", showing=True), "the Revoke button"
    )
    wait_for(lambda: revoke.getState().contains(pyatspi.STATE_SENSITIVE), "Revoke sensitive")
    do_action(revoke)
    wait_for(lambda: find(editor, "status bar", NO_DELEGATES), "the empty status bar again")
    screenshot("delegates-revoked")
    say(f"page: {NO_DELEGATES!r} again")

    wait_for(
        lambda: shared_with(server, "Calendar", calendar_id, principal) is None,
        "the principal gone from the calendar's shareWith",
    )
    wait_for(
        lambda: shared_with(server, "AddressBook", book_id, principal) is None,
        "the principal gone from the address book's shareWith",
    )
    say("server: both collections no longer share with the principal")

    close_with_cancel(editor)
    expect_no_errors()


main(setup, run)
