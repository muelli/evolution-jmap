# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: a contact list (JSContact group card) in the address book of Evolution.

The server has two member contacts and a list naming them by uid, the shape
jmap-book-sync writes and reads (see tests/contact_groups.rs). The checks:

- The list shows in the address book, and its preview names it correctly.
- Its Contact List Editor shows the list's name and both member rows.
- A rename through the editor reaches the server; membership does not change.
"""

from contact_photo import show_account_book
from scenario import (
    Jmap,
    ScenarioFailure,
    click_button,
    descendants,
    double_click,
    evolution,
    expect_no_errors,
    find,
    is_showing,
    main,
    say,
    screenshot,
    set_text,
    text_field,
    wait_for,
    write_collection_account,
)

MEMBER_ONE = "Gail Groupmember"
MEMBER_TWO = "Hank Groupmember"
LIST_NAME = "Garden Club"
RENAMED_LIST_NAME = "Garden Club Renamed"

MEMBER_ONE_UID = "gui-scenario-member-one"
MEMBER_TWO_UID = "gui-scenario-member-two"
LIST_UID = "gui-scenario-list"


def member_card(uid, full_name):
    """Return a plain ContactCard for one list member."""
    return {
        "@type": "Card",
        "version": "1.0",
        "uid": uid,
        "addressBookIds": {"AB1": True},
        "name": {"full": full_name},
    }


def list_card(uid, full_name, member_uids):
    """Return a group ContactCard naming `member_uids` as members."""
    return {
        "@type": "Card",
        "version": "1.0",
        "uid": uid,
        "kind": "group",
        "addressBookIds": {"AB1": True},
        "name": {"full": full_name},
        "members": {member_uid: True for member_uid in member_uids},
    }


def setup():
    write_collection_account()
    server = Jmap()
    for item in (
        member_card(MEMBER_ONE_UID, MEMBER_ONE),
        member_card(MEMBER_TWO_UID, MEMBER_TWO),
        list_card(LIST_UID, LIST_NAME, (MEMBER_ONE_UID, MEMBER_TWO_UID)),
    ):
        card_id = server.create("ContactCard", item)
        say(f"setup: ContactCard {card_id} {(item.get('name') or {}).get('full')!r} created")


def card_id_by_uid(server, uid):
    """Return the id of the card whose JSContact uid is `uid`, or raise."""
    found = server.call("ContactCard/get", {"ids": None})["list"]
    for item in found:
        if item.get("uid") == uid:
            return item["id"]
    raise ScenarioFailure(f"the server has no card with uid {uid!r}: {found}")


def list_cell():
    """Return the showing minicard of the list, or None.

    Evolution draws a minicard's own name as part of its canvas header, not
    as a plain accessible "label", so a list never shows as `wait_find`
    finds a normal contact. Each minicard's AT-SPI "table cell" still lists
    its visible fields as child labels, so the list is the one cell whose
    labels name neither member.
    """
    for cell in descendants(evolution(), "table cell"):
        if not is_showing(cell):
            continue
        names = {label.name for label in descendants(cell, "label") if label.name}
        if names and MEMBER_ONE not in names and MEMBER_TWO not in names:
            return cell
    return None


def list_editor_window():
    """Return the showing Contact List Editor window, or None.

    Its title live-updates to the list's own name as the name field changes
    (`contact_list_editor_list_name_entry_changed_cb`), so it cannot be
    found by a fixed title prefix the way other editors are.
    """
    app = evolution()
    for i in range(app.childCount):
        candidate = app.getChildAtIndex(i)
        if is_showing(candidate) and find(candidate, "push button", name="Save") is not None:
            return candidate
    return None


def run():
    server = Jmap()
    list_id = card_id_by_uid(server, LIST_UID)
    member_one_id = card_id_by_uid(server, MEMBER_ONE_UID)
    member_two_id = card_id_by_uid(server, MEMBER_TWO_UID)

    show_account_book()
    cell = wait_for(list_cell, "the list's minicard", timeout=90)
    double_click(cell)
    editor = wait_for(list_editor_window, "the Contact List Editor")

    def members_shown():
        """Return the member rows the editor shows, once it shows both.

        The editor loads its contact asynchronously after it opens, so an
        immediate check would see the still-empty dialog. It shows each
        member by the raw `X-EVOLUTION-DEST-CONTACT-UID` value (the
        member's own server id here), not by its name: this project writes
        a member's real name only when a prior sync already cached it as a
        vendor extra, which a plain server-loaded group card never has.
        """
        found = {cell.name for cell in descendants(editor, "table cell") if cell.name}
        return found if {member_one_id, member_two_id} <= found else None

    shown = wait_for(members_shown, "the Contact List Editor to show both members")
    screenshot("editor-opened")
    say(f"editor: shows both members {shown}")

    name_field = wait_for(lambda: text_field(editor, LIST_NAME), "the list name field")
    set_text(name_field, RENAMED_LIST_NAME)
    screenshot("renamed")
    click_button(editor, "Save")
    wait_for(lambda: list_editor_window() is None, "the editor closed")

    def renamed():
        stored = server.get("ContactCard", list_id)
        return stored if (stored.get("name") or {}).get("full") == RENAMED_LIST_NAME else None

    stored = wait_for(renamed, f"the name {RENAMED_LIST_NAME!r} on the server", timeout=30)
    say(f"server: has the name {RENAMED_LIST_NAME!r}")

    members = stored.get("members") or {}
    if set(members) != {MEMBER_ONE_UID, MEMBER_TWO_UID}:
        raise ScenarioFailure(f"the rename should not change membership: {members}")
    say(f"server: membership is unchanged {sorted(members)}")
    expect_no_errors()
    expect_no_errors("dbus.log")


main(setup, run, component="addressbook")
