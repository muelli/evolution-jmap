# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: going offline while the Server Filters page is open (ROADMAP
item 180(c), 182(b)'s "server-filters-offline-while-open" row).

Before this fix, going offline while the page was open left every control
sensitive and still reaching for the server; a write then failed late with
a network error. The page now watches the shell's online state directly:
going offline makes the script list and the text editor insensitive at
once, keeps the unsaved text in the buffer, and shows the page's own
offline note. Coming back online restores the controls and the note,
without dropping the unsaved edit.

This scenario edits the seeded script's text, then toggles File > Work
Offline with the page still open, the case `settings-pages-offline.py`
does not cover (that scenario opens the page already offline).

The edit is a line the local Sieve parser rejects on its own (item 171b),
the same `LOCAL_SYNTAX_ERROR_TEXT` shape `server-filters.py` step 4a uses.
A valid edit's own debounce (item 180(a)) can save it to the server in
well under a second, before this scenario gets to toggle offline; an
edit the local parser never accepts is never written, online or offline,
so the "still unsaved" premise holds regardless of that race.
"""

import json
import urllib.request

from scenario import (
    Jmap,
    ScenarioFailure,
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
    set_text,
    text_field,
    text_of,
    wait_for,
    window,
    write_collection_mail_account,
)

ACCOUNT = "JMAP scenario mail"
SIEVE_CAPABILITY = "urn:ietf:params:jmap:sieve"
EDITOR = "Account Editor"
PAGE = "Server Filters"
LEGACY_NAME = "legacy"
LEGACY_TEXT = "keep;\n"
# A line the local Sieve parser rejects on its own (item 171b): an
# unterminated block. Never written, online or offline, with no Check
# click and no debounce race (see the module docstring).
EDITED_TEXT = "if true {"
OFFLINE_NOTE = "You are offline. Filters cannot be read or changed until you reconnect."
# Well under jmap-client's 30-second connect timeout.
OFFLINE_MESSAGE_TIMEOUT = 10
# The X keysym of the Menu key.
MENU_KEY = 0xFF67


# JMAP helpers, the same shape `server-filters.py` uses (no blob upload or
# download in the shared `Jmap` class).


def _session(server):
    with urllib.request.urlopen(f"{server.url}/.well-known/jmap", timeout=5) as response:
        return json.load(response)


def upload_sieve(server, text):
    url = _session(server)["uploadUrl"].replace("{accountId}", server.account_id)
    request = urllib.request.Request(
        url, data=text.encode(), headers={"Content-Type": "application/sieve"}, method="POST"
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        return json.load(response)["blobId"]


def download_sieve(server, blob_id):
    url = (
        _session(server)["downloadUrl"]
        .replace("{accountId}", server.account_id)
        .replace("{blobId}", blob_id)
        .replace("{name}", "script.sieve")
        .replace("{type}", "application/sieve")
    )
    with urllib.request.urlopen(url, timeout=10) as response:
        return response.read().decode()


def create_script(server, name, text):
    """Create a script with `name` and `text`, inactive. Return its id."""
    blob_id = upload_sieve(server, text)
    created = server.call(
        "SieveScript/set",
        {"create": {"s": {"name": name, "blobId": blob_id}}},
        extra_using=(SIEVE_CAPABILITY,),
    )
    if "s" not in (created.get("created") or {}):
        raise ScenarioFailure(f"could not create the script {name!r}: {created}")
    return created["created"]["s"]["id"]


def sieve_scripts(server):
    return server.call("SieveScript/get", {"ids": None}, extra_using=(SIEVE_CAPABILITY,))["list"]


def setup():
    write_collection_mail_account(ACCOUNT)
    server = Jmap()
    create_script(server, LEGACY_NAME, LEGACY_TEXT)
    say(f"setup: a plain script {LEGACY_NAME!r}")


# AT-SPI helpers.


def open_page():
    """Open the account editor of ACCOUNT on the Server Filters page."""
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
    """Wait until the page has loaded and return the script list's Add button."""
    add = wait_for(
        lambda: find(editor, "push button", "Add…", showing=True), "a showing script Add button"
    )
    wait_for(lambda: add.getState().contains(pyatspi.STATE_SENSITIVE), "the page loaded")
    return add


def select_script(editor, name):
    cell = wait_for(lambda: find(editor, "table cell", name), f"{name!r} in the script list")
    cell.queryComponent().grabFocus()


def close_with_cancel(editor):
    cancel = wait_for(
        lambda: find(editor, "push button", "Cancel", showing=True), "the editor's Cancel button"
    )
    do_action(cancel)
    wait_for(lambda: window(EDITOR) is None, "the account editor closes")


def has_error_underline(source_view, offset=0):
    """True if the local-parse error tag (item 171b) covers `offset`."""
    attributes, _start, _end = source_view.queryText().getAttributes(offset)
    return "underline:error" in attributes.split(";")


def run():
    server = Jmap()

    editor = open_page()
    add = loaded(editor)
    select_script(editor, LEGACY_NAME)
    source_view = wait_for(
        lambda: text_field(editor, LEGACY_TEXT), f"the text view showing {LEGACY_TEXT!r}"
    )
    say(f"page: {LEGACY_NAME!r} loaded online")

    set_text(source_view, EDITED_TEXT)
    wait_for(lambda: has_error_underline(source_view), "a local error underline")
    say("page: edited the script text to a local syntax error, never written")
    screenshot("edited-online")

    menu_item("File/Work Offline")
    say("menu: File > Work Offline, with the page still open")

    wait_for(
        lambda: find(editor, "status bar", OFFLINE_NOTE),
        "the offline note on the Server Filters page",
        timeout=OFFLINE_MESSAGE_TIMEOUT,
    )
    say(f"page: {OFFLINE_NOTE!r}")
    wait_for(
        lambda: not add.getState().contains(pyatspi.STATE_SENSITIVE), "Add goes insensitive"
    )
    wait_for(
        lambda: not source_view.getState().contains(pyatspi.STATE_SENSITIVE),
        "the script text view goes insensitive",
    )
    if text_of(source_view) != EDITED_TEXT:
        raise ScenarioFailure("the unsaved edit was lost when the page went offline")
    say("page: read only, the unsaved edit is kept")
    screenshot("offline-while-open")

    menu_item("File/Work Online")
    say("menu: File > Work Online")

    wait_for(lambda: add.getState().contains(pyatspi.STATE_SENSITIVE), "Add re-enables")
    wait_for(
        lambda: source_view.getState().contains(pyatspi.STATE_SENSITIVE),
        "the script text view re-enables",
    )
    wait_for(
        lambda: find(editor, "status bar", OFFLINE_NOTE) is None, "the offline note clears"
    )
    if text_of(source_view) != EDITED_TEXT:
        raise ScenarioFailure("the unsaved edit was lost when the page came back online")
    say("page: editable again online, the unsaved edit is still kept")
    screenshot("back-online")

    close_with_cancel(editor)
    scripts = {script["name"]: script for script in sieve_scripts(server)}
    if download_sieve(server, scripts[LEGACY_NAME]["blobId"]) != LEGACY_TEXT:
        raise ScenarioFailure("the unsaved edit reached the server despite Cancel")
    say("server: the unsaved edit never reached the server")

    expect_no_errors()


main(setup, run)
