# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: the Delegates page of the account editor (ROADMAP item 164).

1. The page loads with no delegate: the status bar says so.
2. Alice clicks Add, fills the principal id and the three level combos, and
   confirms. The server's calendar, address book and Inbox all carry the
   chosen rights for that principal, and the page lists the new row.
3. Alice selects the row and clicks Change (item 193(a)). The dialog opens
   prefilled with the row's principal (fixed, not editable) and its current
   levels. She changes all three levels and confirms; the server's shares
   and the page's row both show the new levels.
4. Alice selects the row and clicks Revoke. The server no longer shares
   the calendar, the address book or the Inbox with that principal, and the
   page shows the empty status bar again.

Step 2 and 3 also cover the Mail row (item 193(b)): the mail level combo
grants and changes a share of the account's Inbox, the subtree
`commit_mail_delegation` writes to.

The Add dialog's field takes a principal id or an address (ROADMAP item
185(b)): the mock seeds "bob@example.com" as a real principal so this
scenario's typed address resolves the same way a real server's does.

With `--live` (item 182(b)), the principal is `harness/stalwart/
seed-manual-test.py`'s own seeded "bob" account id, read from the --state
file, since a real server refuses an address that is not also a valid id.
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
    text_of,
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
CHANGE_DIALOG = "Change Delegate"
# In document order: [calendar level, address book level, mail level].
CALENDAR_LEVEL_ROW = 3  # CalendarDelegationLevel::Author
BOOK_LEVEL_ROW = 1  # AddressBookDelegationLevel::Reviewer
MAIL_LEVEL_ROW = 3  # MailDelegationLevel::Editor
CALENDAR_LEVEL_NAME = "Author"
BOOK_LEVEL_NAME = "Reviewer"
MAIL_LEVEL_NAME = "Editor"
# Item 193(a): the Change dialog changes every level further.
NEW_CALENDAR_LEVEL_ROW = 4  # CalendarDelegationLevel::Editor
NEW_BOOK_LEVEL_ROW = 2  # AddressBookDelegationLevel::Author
NEW_MAIL_LEVEL_ROW = 1  # MailDelegationLevel::Reviewer
NEW_CALENDAR_LEVEL_NAME = "Editor"
NEW_BOOK_LEVEL_NAME = "Author"
NEW_MAIL_LEVEL_NAME = "Reviewer"


def _server():
    if LIVE_USER:
        return Jmap(user=LIVE_USER, password=LIVE_PASSWORD, rebase=True)
    return Jmap()


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
    # Ask for shareWith by name: Stalwart's Mailbox/get omits it from the
    # "all properties" default (ROADMAP item 193, STALWART-RFC-FINDINGS 30).
    found = server.get(type_name, object_id, properties=["shareWith"])
    if found is None:
        raise ScenarioFailure(f"the server has no {type_name} {object_id}")
    return (found.get("shareWith") or {}).get(principal)


def run():
    principal = PRINCIPAL
    if LIVE_USER:
        authenticate(LIVE_PASSWORD, remember=True, label="authentication-request")
        principal = live_state()["accounts"]["bob"]
    server = _server()
    # `principal` is what the user types; the server resolves it to a
    # principal id (`io::resolve_principal`), the key `shareWith` and the
    # Delegates list actually carry.
    principal_id = principal if LIVE_USER else server.resolve_principal_id(principal)
    principal_display = server.principal_display(principal_id)
    calendars = server.names("Calendar")
    books = server.names("AddressBook")
    calendar_id = next(iter(calendars))
    book_id = next(iter(books))
    inbox_id = server.mailbox_id("inbox")

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
    if len(combos) != 3:
        raise ScenarioFailure(f"the Add dialog has {len(combos)} combo boxes, not 3")
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
    combos[2].querySelection().selectChild(MAIL_LEVEL_ROW)
    wait_for(
        lambda: selected_item_name(combos[2]) == MAIL_LEVEL_NAME,
        f"the mail level combo shows {MAIL_LEVEL_NAME!r}",
    )
    screenshot("delegates-add-filled")
    click_button(dialog, "Add")
    wait_for(lambda: window(ADD_DIALOG) is None, "the Add Delegate dialog closes")
    wait_for(lambda: find(editor, "table cell", principal_display), f"{principal_display!r} listed")
    wait_for(lambda: find(editor, "table cell", CALENDAR_LEVEL_NAME), "its calendar level listed")
    wait_for(lambda: find(editor, "table cell", BOOK_LEVEL_NAME), "its address book level listed")
    wait_for(lambda: find(editor, "table cell", MAIL_LEVEL_NAME), "its mail level listed")
    say(
        f"page: {principal_display!r} listed with "
        f"{CALENDAR_LEVEL_NAME!r}/{BOOK_LEVEL_NAME!r}/{MAIL_LEVEL_NAME!r}"
    )
    screenshot("delegates-added")

    calendar_rights = wait_for(
        lambda: shared_with(server, "Calendar", calendar_id, principal_id),
        f"{principal_id!r} in the calendar's shareWith",
    )
    if not calendar_rights.get("mayWriteOwn"):
        raise ScenarioFailure(f"the calendar share is not Author: {calendar_rights}")
    book_rights = wait_for(
        lambda: shared_with(server, "AddressBook", book_id, principal_id),
        f"{principal_id!r} in the address book's shareWith",
    )
    if not book_rights.get("mayRead") or book_rights.get("mayWrite"):
        raise ScenarioFailure(f"the address book share is not Reviewer: {book_rights}")
    mail_rights = wait_for(
        lambda: shared_with(server, "Mailbox", inbox_id, principal_id),
        f"{principal_id!r} in the Inbox's shareWith",
    )
    if not mail_rights.get("mayRemoveItems"):
        raise ScenarioFailure(f"the mail share is not Editor: {mail_rights}")
    say("server: the calendar, the address book and the Inbox share at the chosen levels")

    # 3. Change changes every level (item 193(a)).
    row = wait_for(lambda: find(editor, "table cell", principal_display), f"{principal_display!r} listed")
    row.queryComponent().grabFocus()
    change = wait_for(
        lambda: find(editor, "push button", "Change…", showing=True), "the Change button"
    )
    wait_for(lambda: change.getState().contains(pyatspi.STATE_SENSITIVE), "Change sensitive")
    click_at(change)
    dialog = wait_for(lambda: window(CHANGE_DIALOG), "the Change Delegate dialog")
    entry = wait_for(lambda: find(dialog, "text"), "the principal entry")
    if text_of(entry) != principal_display:
        raise ScenarioFailure(
            f"the Change dialog's principal entry shows {text_of(entry)!r}, not {principal_display!r}"
        )
    if entry.getState().contains(pyatspi.STATE_SENSITIVE):
        raise ScenarioFailure("the Change dialog's principal entry accepts editing")
    combos = list(descendants(dialog, "combo box"))
    if len(combos) != 3:
        raise ScenarioFailure(f"the Change dialog has {len(combos)} combo boxes, not 3")
    wait_for(
        lambda: selected_item_name(combos[0]) == CALENDAR_LEVEL_NAME,
        f"the calendar level combo starts at {CALENDAR_LEVEL_NAME!r}",
    )
    wait_for(
        lambda: selected_item_name(combos[1]) == BOOK_LEVEL_NAME,
        f"the address book level combo starts at {BOOK_LEVEL_NAME!r}",
    )
    wait_for(
        lambda: selected_item_name(combos[2]) == MAIL_LEVEL_NAME,
        f"the mail level combo starts at {MAIL_LEVEL_NAME!r}",
    )
    say(
        f"dialog: prefilled with {principal_display!r}, "
        f"{CALENDAR_LEVEL_NAME!r}/{BOOK_LEVEL_NAME!r}/{MAIL_LEVEL_NAME!r}"
    )
    combos[0].querySelection().selectChild(NEW_CALENDAR_LEVEL_ROW)
    wait_for(
        lambda: selected_item_name(combos[0]) == NEW_CALENDAR_LEVEL_NAME,
        f"the calendar level combo shows {NEW_CALENDAR_LEVEL_NAME!r}",
    )
    combos[1].querySelection().selectChild(NEW_BOOK_LEVEL_ROW)
    wait_for(
        lambda: selected_item_name(combos[1]) == NEW_BOOK_LEVEL_NAME,
        f"the address book level combo shows {NEW_BOOK_LEVEL_NAME!r}",
    )
    combos[2].querySelection().selectChild(NEW_MAIL_LEVEL_ROW)
    wait_for(
        lambda: selected_item_name(combos[2]) == NEW_MAIL_LEVEL_NAME,
        f"the mail level combo shows {NEW_MAIL_LEVEL_NAME!r}",
    )
    screenshot("delegates-change-filled")
    click_button(dialog, "Change")
    wait_for(lambda: window(CHANGE_DIALOG) is None, "the Change Delegate dialog closes")
    wait_for(
        lambda: find(editor, "table cell", NEW_CALENDAR_LEVEL_NAME),
        "its new calendar level listed",
    )
    wait_for(
        lambda: find(editor, "table cell", NEW_BOOK_LEVEL_NAME), "its new address book level listed"
    )
    wait_for(lambda: find(editor, "table cell", NEW_MAIL_LEVEL_NAME), "its new mail level listed")
    say(
        f"page: {principal_display!r} listed with "
        f"{NEW_CALENDAR_LEVEL_NAME!r}/{NEW_BOOK_LEVEL_NAME!r}/{NEW_MAIL_LEVEL_NAME!r}"
    )
    screenshot("delegates-changed")

    calendar_rights = wait_for(
        lambda: shared_with(server, "Calendar", calendar_id, principal_id),
        f"{principal_id!r} in the calendar's shareWith",
    )
    if not calendar_rights.get("mayWriteAll"):
        raise ScenarioFailure(f"the calendar share is not Editor: {calendar_rights}")
    book_rights = wait_for(
        lambda: shared_with(server, "AddressBook", book_id, principal_id),
        f"{principal_id!r} in the address book's shareWith",
    )
    if not book_rights.get("mayWrite"):
        raise ScenarioFailure(f"the address book share is not Author: {book_rights}")
    mail_rights = wait_for(
        lambda: shared_with(server, "Mailbox", inbox_id, principal_id),
        f"{principal_id!r} in the Inbox's shareWith",
    )
    if not mail_rights.get("mayReadItems") or mail_rights.get("mayAddItems"):
        raise ScenarioFailure(f"the mail share is not Reviewer: {mail_rights}")
    say("server: the calendar and the address book carry the raised levels, the mail share the new one")

    # 4. Revoke clears every share.
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
        lambda: shared_with(server, "Calendar", calendar_id, principal_id) is None,
        "the principal gone from the calendar's shareWith",
    )
    wait_for(
        lambda: shared_with(server, "AddressBook", book_id, principal_id) is None,
        "the principal gone from the address book's shareWith",
    )
    wait_for(
        lambda: shared_with(server, "Mailbox", inbox_id, principal_id) is None,
        "the principal gone from the Inbox's shareWith",
    )
    say("server: the calendar, the address book and the Inbox no longer share with the principal")

    close_with_cancel(editor)
    expect_no_errors()


main(setup, run)
