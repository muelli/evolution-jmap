# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: event categories set, renamed and cleared in the event editor.

ROADMAP item 177(a). The event editor hides its Categories row by default
(`org.gnome.evolution.calendar:editor-show-categories`, default false);
`ci/gui-scenario/run.sh` turns it on for the "calendar" component so this
scenario can reach it.

The row is a plain, directly editable `GtkEntry` with no accessible name and
no labelled-by relation: its caption is a "_Categories…" button, not a
`GtkLabel`, so GTK never wires the automatic relation `contact-editor.ui`
sets up for the same kind of row. The button opens a picker dialog through
`gtk_dialog_run`, a nested main loop; a throwaway probe confirmed the whole
Evolution application drops out of the AT-SPI desktop for as long as that
loop runs (`evolution()` returns `None`), so the dialog itself is not
reachable here, the same shape as the Account Editor's Close button in
`docs/gui-scenario-tests.md`. The field is still reachable directly: typing
into it works with no dialog involved, so this scenario types into it and
never clicks the button. The field is found by position, not name: a
GtkGrid's AT-SPI child order puts each row's edit widget immediately before
its own label or button (confirmed empirically: setting the sibling just
before "Categories…" landed on the server's `keywords`; the sibling just
after it is labelled "Web page:" and never did).

Checks:
- Typing "Family, Colleagues" and saving reaches the server as two keywords.
- Renaming to "Work" only replaces the set: Family and Colleagues are gone.
- Clearing the field and saving removes the keywords property entirely.
"""

import time

from scenario import (
    Jmap,
    click_button,
    descendants,
    double_click,
    expect_no_errors,
    main,
    pyatspi,
    say,
    set_text,
    text_of,
    wait_find,
    wait_for,
    window,
    write_collection_account,
)

TITLE = "Probe Categories Event"
EVENT_UID = "gui-scenario-event-categories"
EDITOR = "Appointment"
END_KEY = 0xFF57
BACKSPACE_KEY = 0xFF08


def event():
    return {
        "@type": "Event",
        "version": "2.0",
        "uid": EVENT_UID,
        "calendarIds": {"CAL1": True},
        "title": TITLE,
        "start": time.strftime("%Y-%m-%dT10:00:00"),
        "timeZone": "Etc/UTC",
        "duration": "PT1H",
    }


def setup():
    write_collection_account()
    Jmap().create("CalendarEvent", event())


def event_id(server):
    ids = server.call("CalendarEvent/query", {})["ids"]
    matches = [i for i in ids if server.get("CalendarEvent", i).get("uid") == EVENT_UID]
    if len(matches) != 1:
        raise RuntimeError(f"expected 1 event with uid {EVENT_UID!r}, found {matches}")
    return matches[0]


def categories_field(editor):
    """Return the event editor's Categories entry, or None.

    It has no accessible name or labelled-by relation; it is the sibling
    just before the "Categories…" button in their shared GtkGrid.
    """
    for button in descendants(editor, "push button"):
        if button.name != "Categories…":
            continue
        parent = button.parent
        index = button.getIndexInParent()
        if parent is None or index <= 0:
            return None
        return parent.getChildAtIndex(index - 1)
    return None


def clear_text(node):
    """Empty an entry with real key events: End, then one Backspace per character.

    A plain `deleteText` or `setTextContents` call may not dirty the editor's
    Save button the way a real edit does, the same reason `contact-
    categories.py` uses this instead of `set_text`'s own delete call.
    """
    node.queryComponent().grabFocus()
    pyatspi.Registry.generateKeyboardEvent(END_KEY, None, pyatspi.KEY_SYM)
    for _ in range(len(text_of(node))):
        pyatspi.Registry.generateKeyboardEvent(BACKSPACE_KEY, None, pyatspi.KEY_SYM)
    wait_for(lambda: text_of(node) == "", "the field is empty")
    say("cleared the field")


def open_editor():
    item = wait_find(
        "Calendar Event", prefix=f"Calendar Event: Summary is {TITLE}", showing=True, timeout=90
    )
    double_click(item)
    return wait_for(lambda: window(EDITOR), f"the editor {EDITOR!r}")


def save_and_close(editor):
    click_button(editor, "Save and Close")
    wait_for(lambda: window(EDITOR) is None, "the editor closed")


def keywords_of(server, eid):
    return (server.get("CalendarEvent", eid) or {}).get("keywords")


def wait_for_keywords(server, eid, expected):
    def matches():
        found = keywords_of(server, eid)
        return found if found == expected else None

    found = wait_for(matches, f"the server keywords {expected!r}", timeout=30)
    say(f"server: keywords {found!r}")
    return found


def wait_for_no_keywords(server, eid):
    def cleared():
        return True if not keywords_of(server, eid) else None

    wait_for(cleared, "no keywords on the server", timeout=30)
    say("server: no keywords")


def run():
    server = Jmap()
    eid = event_id(server)

    editor = open_editor()
    field = wait_for(lambda: categories_field(editor), "the Categories field")
    set_text(field, "Family, Colleagues")
    save_and_close(editor)
    wait_for_keywords(server, eid, {"Family": True, "Colleagues": True})

    editor = open_editor()
    field = wait_for(lambda: categories_field(editor), "the Categories field")
    set_text(field, "Work")
    save_and_close(editor)
    wait_for_keywords(server, eid, {"Work": True})

    editor = open_editor()
    field = wait_for(lambda: categories_field(editor), "the Categories field")
    clear_text(field)
    save_and_close(editor)
    wait_for_no_keywords(server, eid)

    expect_no_errors()
    expect_no_errors("dbus.log")


main(setup, run, component="calendar")
