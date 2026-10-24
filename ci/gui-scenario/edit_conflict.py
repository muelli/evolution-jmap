# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""An edit in Evolution and an edit on the server of different fields.

The scenarios edit-conflict-event and edit-conflict-contact call run_scenario.
The steps and checks:

1. Open the item in its editor. The editor shows field A and field B.
2. The test changes field B on the server. Evolution gets the change through
   push and refreshes its cache. The open editor still shows the old field B.
3. Change field A in the editor and save.
4. The server has the new field A from Evolution and the new field B from the
   server.
"""

import time

from scenario import (
    Jmap,
    ScenarioFailure,
    click_button,
    descendants,
    double_click,
    evolution,
    expect_no_errors,
    is_showing,
    log_count,
    say,
    screenshot,
    set_text,
    text_field,
    wait_find,
    wait_for,
    window,
    write_collection_account,
)


class Kind:
    """What differs between an event and a contact."""

    def __init__(self, type_name, item, editor, save, refreshed, edited, server):
        self.type_name = type_name
        # The JMAP object of the setup.
        self.item = item
        # The name of the editor window starts with this text.
        self.editor = editor
        self.save = save
        # The log line of the refresh, in dbus.log.
        self.refreshed = refreshed
        # (shown before, typed in Evolution, read from the server)
        self.edited = edited
        # (shown before, patch on the server, read from the server)
        self.server = server

    def created(self):
        """Return the id of the item of the setup on the server."""
        jmap = Jmap()
        ids = jmap.call(f"{self.type_name}/query", {})["ids"]
        if len(ids) != 1:
            raise ScenarioFailure(f"the server has {len(ids)} {self.type_name} objects, not 1")
        return ids[0]


def _note(card):
    return next(iter((card.get("notes") or {}).values()), {}).get("note")


EVENT = Kind(
    "CalendarEvent",
    {
        "@type": "Event",
        "version": "2.0",
        "uid": "edit-conflict-event",
        "calendarIds": {"CAL1": True},
        "title": "Team sync",
        "description": "Agenda 1",
        "start": time.strftime("%Y-%m-%dT10:00:00"),
        "timeZone": "Etc/UTC",
        "duration": "PT1H",
    },
    "Appointment",
    "Save and Close",
    "reported calendar changes",
    ("Team sync", "Team sync moved", lambda event: event.get("title")),
    ("Agenda 1", {"description": "Agenda 2"}, lambda event: event.get("description")),
)
CONTACT = Kind(
    "ContactCard",
    {
        "@type": "Card",
        "version": "1.0",
        "uid": "edit-conflict-contact",
        "addressBookIds": {"AB1": True},
        "name": {"full": "Jane Example"},
        "notes": {"n1": {"note": "Note 1"}},
    },
    "Contact Editor",
    "Save",
    "reported address book changes",
    # The Save of the contact editor in Evolution 3.52 stays disabled after an
    # edit of the nickname alone, so the scenario edits the full name.
    ("Jane Example", "Jane Q Example", lambda card: (card.get("name") or {}).get("full")),
    ("Note 1", {"notes/n1/note": "Note 2"}, _note),
)


def setup(kind):
    write_collection_account()
    item_id = Jmap().create(kind.type_name, kind.item)
    say(f"setup: collection account written, {kind.type_name} {item_id} created")


def _open_item(kind):
    """Show the item of the setup and open it in its editor."""
    if kind is EVENT:
        item = wait_find(
            "Calendar Event", prefix="Calendar Event: Summary is Team sync", showing=True, timeout=90
        )
    else:
        # The address book of the account, not the local "Personal" above it.
        def jmap_book():
            cells = [
                cell
                for cell in descendants(evolution(), "table cell")
                if cell.name == "Personal" and is_showing(cell)
            ]
            return cells[-1] if len(cells) > 1 else None

        wait_for(jmap_book, "the address book of the account").queryComponent().grabFocus()
        item = wait_find("label", "Jane Example", showing=True)
    screenshot("before-edit")
    double_click(item)
    editor = wait_for(lambda: window(kind.editor), f"the editor {kind.editor!r}")
    say(f"editor {kind.editor!r} is open")
    return editor


def run(kind):
    jmap = Jmap()
    item_id = kind.created()
    shown, typed, read_edited = kind.edited
    server_shown, server_patch, read_server = kind.server

    editor = _open_item(kind)
    field = wait_for(lambda: text_field(editor, shown), f"the field with {shown!r}")
    wait_for(lambda: text_field(editor, server_shown), f"the field with {server_shown!r}")
    screenshot("editor-open")

    refreshes = log_count(kind.refreshed, "dbus.log")
    jmap.update(kind.type_name, item_id, server_patch)
    expected_server = read_server(jmap.get(kind.type_name, item_id))
    say(f"server: changed to {expected_server!r}")
    wait_for(
        lambda: log_count(kind.refreshed, "dbus.log") > refreshes,
        "the refresh after the server change",
        timeout=30,
    )
    say("Evolution refreshed its cache")
    if text_field(editor, server_shown) is None:
        raise ScenarioFailure(f"the editor no longer shows {server_shown!r}")
    say(f"the open editor still shows {server_shown!r}")

    set_text(field, typed)
    screenshot("edited")
    click_button(editor, kind.save)
    wait_for(lambda: window(kind.editor) is None, "the editor closed")

    def edited_on_server():
        item = jmap.get(kind.type_name, item_id)
        return item if read_edited(item) == typed else None

    stored = wait_for(edited_on_server, f"the edit {typed!r} on the server")
    screenshot("saved")
    say(f"server: has the edit {typed!r}")
    if read_server(stored) != expected_server:
        raise ScenarioFailure(
            f"the save overwrote the server change: {read_server(stored)!r}, "
            f"expected {expected_server!r}"
        )
    say(f"server: kept its own change {expected_server!r}")
    expect_no_errors()
    expect_no_errors("dbus.log")
