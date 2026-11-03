# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: contact categories set, renamed and cleared in the contact editor.

ROADMAP item 177(a). The contact editor's "Categories" row is a plain,
directly editable `GtkEntry` (`contact-editor.ui`'s `entry-categories`); its
own accessible name stays empty, named only through a labelled-by relation to
the "Categories..." button beside it, the same shape `mail-flags.py` already
found for the Add Label dialog's name entry, so the field is found that way,
not by its own name or its current content.

Checks:
- Typing "Family, Colleagues" and saving reaches the server as two keywords.
- Renaming to "Work" only replaces the set: Family and Colleagues are gone.
- Clearing the field and saving removes the keywords property entirely.
"""

import contact_photo as cp
from scenario import (
    Jmap,
    click_button,
    descendants,
    double_click,
    expect_no_errors,
    labelled_by_name,
    main,
    pyatspi,
    say,
    set_text,
    text_of,
    wait_find,
    wait_for,
    window,
)

CATEGORIES_BUTTON_NAMES = ("Categories...", "Categories…")

FULL_NAME = "Cara Categories"
CARD_UID = "gui-scenario-categories"
EDITOR = "Contact Editor"
END_KEY = 0xFF57
BACKSPACE_KEY = 0xFF08


def card():
    return {
        "@type": "Card",
        "version": "1.0",
        "uid": CARD_UID,
        "addressBookIds": {"AB1": True},
        "name": {"full": FULL_NAME},
    }


def setup():
    cp.setup([card()])


def categories_field(editor):
    """Return the contact editor's Categories entry, or None.

    On Evolution 3.52.3 it has no accessible name of its own; it is named
    only through a labelled-by relation to the "Categories..." button
    beside it (3.60.2 ships that button's label with one real ellipsis
    character, "Categories…", the same HIG tightening item 174(c) already
    applied to this project's own buttons, so both forms are checked). On
    3.60.2 that relation was not found live, so this also falls back to the
    entry nested in the filler right before the button, the same
    position-based approach calendar-categories.py uses for the event
    editor's own Categories field. Both loops look up the button by name,
    not role: the button's role name is "push button" on some AT-SPI
    versions and "button" on others, the same gap `click_button` already
    works around.
    """
    for node in descendants(editor, "text"):
        if labelled_by_name(node) in CATEGORIES_BUTTON_NAMES:
            return node
    for button in descendants(editor):
        if button.name not in CATEGORIES_BUTTON_NAMES:
            continue
        parent = button.parent
        index = button.getIndexInParent()
        if parent is None or index <= 0:
            return None
        sibling = parent.getChildAtIndex(index - 1)
        if sibling is None:
            return None
        if sibling.getRoleName() == "text":
            return sibling
        return next(descendants(sibling, "text"), None)
    return None


def clear_text(node):
    """Empty an entry with real key events: End, then one Backspace per character.

    A plain `deleteText` or `setTextContents` call may not dirty the editor's
    Save button the way a real edit does (the same reason `set_text` types
    through a synthetic key press rather than setting the content directly).
    """
    node.queryComponent().grabFocus()
    pyatspi.Registry.generateKeyboardEvent(END_KEY, None, pyatspi.KEY_SYM)
    for _ in range(len(text_of(node))):
        pyatspi.Registry.generateKeyboardEvent(BACKSPACE_KEY, None, pyatspi.KEY_SYM)
    wait_for(lambda: text_of(node) == "", "the field is empty")
    say("cleared the field")


def open_editor():
    item = wait_find("label", FULL_NAME, showing=True)
    double_click(item)
    return wait_for(lambda: window(EDITOR), "the contact editor")


def save_and_close(editor):
    click_button(editor, "Save")
    wait_for(lambda: window(EDITOR) is None, "the editor closed")


def keywords_of(server, card_id):
    return (server.get("ContactCard", card_id) or {}).get("keywords")


def wait_for_keywords(server, card_id, expected):
    def matches():
        found = keywords_of(server, card_id)
        return found if found == expected else None

    found = wait_for(matches, f"the server keywords {expected!r}", timeout=30)
    say(f"server: keywords {found!r}")
    return found


def wait_for_no_keywords(server, card_id):
    def cleared():
        return True if not keywords_of(server, card_id) else None

    wait_for(cleared, "no keywords on the server", timeout=30)
    say("server: no keywords")


def run():
    server = Jmap()
    card_id = cp.card_ids(server)[FULL_NAME]
    cp.show_account_book()

    editor = open_editor()
    field = wait_for(lambda: categories_field(editor), "the Categories field")
    set_text(field, "Family, Colleagues")
    save_and_close(editor)
    wait_for_keywords(server, card_id, {"Family": True, "Colleagues": True})

    editor = open_editor()
    field = wait_for(lambda: categories_field(editor), "the Categories field")
    set_text(field, "Work")
    save_and_close(editor)
    wait_for_keywords(server, card_id, {"Work": True})

    editor = open_editor()
    field = wait_for(lambda: categories_field(editor), "the Categories field")
    clear_text(field)
    save_and_close(editor)
    wait_for_no_keywords(server, card_id)

    expect_no_errors()
    expect_no_errors("dbus.log")


main(setup, run, component="addressbook")
