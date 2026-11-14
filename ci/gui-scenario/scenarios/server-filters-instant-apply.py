# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: Server Filters writes Activate, Add and Remove at once (ROADMAP
item 180(a), 182(b)'s "server-filters-instant-apply" row).

Before this fix, Activate, Deactivate, Add, Rename and Remove only staged a
change; the write happened on the account editor's own OK. This scenario
proves the write already reached the server before OK is ever clicked, by
doing each action and then closing the editor with Cancel instead: a write
that a later Cancel could still undo was never instant in the first place.

`server-filters.py` already covers Activate followed by OK (step 2) and a
plain OK with no change (step 3); this scenario is the one row those do not
cover, the Cancel case.
"""

import json
import urllib.request

from scenario import (
    Jmap,
    ScenarioFailure,
    _name,
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
SIEVE_CAPABILITY = "urn:ietf:params:jmap:sieve"
EDITOR = "Account Editor"
PAGE = "Server Filters"
# The X keysym of the Menu key.
MENU_KEY = 0xFF67

LEGACY_NAME = "legacy"
LEGACY_TEXT = "keep;\n"
MANAGED_NAME = "evolution-jmap"
RULE_NAME = "Invoices"
INVOICES_FOLDER = "Invoices"


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


def folder_paths(server):
    """Return [(id, full path)] of every mailbox, sorted by path."""
    mailboxes = server.call("Mailbox/get", {"ids": None})["list"]
    by_id = {mailbox["id"]: mailbox for mailbox in mailboxes if mailbox.get("id")}

    def path(mailbox, depth=0):
        parent_id = mailbox.get("parentId")
        parent = by_id.get(parent_id) if parent_id else None
        if parent is None or depth > 64:
            return mailbox["name"]
        return f"{path(parent, depth + 1)}/{mailbox['name']}"

    paths = [(mailbox["id"], path(mailbox)) for mailbox in mailboxes]
    paths.sort(key=lambda pair: pair[1])
    return paths


def setup():
    write_collection_mail_account(ACCOUNT)
    server = Jmap()
    create_script(server, LEGACY_NAME, LEGACY_TEXT)
    server.create_mailbox(INVOICES_FOLDER)
    say(f"setup: a plain script {LEGACY_NAME!r} and the {INVOICES_FOLDER!r} mailbox")


# AT-SPI helpers, the same shape `server-filters.py` uses.


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


def script_row_and_content(editor):
    """Return (the script list's row, the page's content box).

    The script list's "Active" column header is unique, so this walks up
    from it: the header's table is the script `GtkTreeView`, its parent is
    the row holding the view and its buttons, and that row's parent is the
    page's own content box, whose first child is the status label. Mirrors
    `server-filters.py`'s `structural_nodes`, for the script list instead
    of the rules list.
    """
    header = wait_for(
        lambda: find(editor, "table column header", "Active"), "the 'Active' column header"
    )
    table = header.parent
    script_row = table.parent
    return script_row, script_row.parent


def script_buttons(editor):
    """Return the script list's own button box, not the rules list's.

    Both lists have a button named "Remove" (one from "Re_move", the other
    from "Rem_ove").
    """
    script_row, _ = script_row_and_content(editor)
    return script_row.getChildAtIndex(1)


def status_text(editor):
    _, content = script_row_and_content(editor)
    return _name(content.getChildAtIndex(0))


def script_button(editor, name):
    buttons_box = script_buttons(editor)
    button = find(buttons_box, "push button", name)
    if button is None:
        raise ScenarioFailure(f"no script list button named {name!r}")
    return button


def rule_buttons(editor):
    """Return the rules list's own button box, not the script list's.

    Both lists have a button named "Add…"; the rules list's "Enabled"
    column header is unique, so this walks up from it (`server-filters.py`'s
    `structural_nodes` does the same, for both boxes at once).
    """
    header = wait_for(
        lambda: find(editor, "table column header", "Enabled"), "the 'Enabled' column header"
    )
    table = header.parent
    rules_row = table.parent
    return rules_row.getChildAtIndex(1)


def rule_button(editor, name):
    buttons_box = rule_buttons(editor)
    button = find(buttons_box, "push button", name)
    if button is None:
        raise ScenarioFailure(f"no rule list button named {name!r}")
    return button


def script_cell(editor, text):
    return find(editor, "table cell", text)


def select_script(editor, name):
    cell = wait_for(lambda: script_cell(editor, name), f"{name!r} in the script list")
    cell.queryComponent().grabFocus()


def selected_item_name(combo):
    selection = combo.querySelection()
    if selection.nSelectedChildren < 1:
        return ""
    return _name(selection.getSelectedChild(0))


def close_with_cancel(editor):
    cancel = wait_for(
        lambda: find(editor, "push button", "Cancel", showing=True), "the editor's Cancel button"
    )
    do_action(cancel)
    wait_for(lambda: window(EDITOR) is None, "the account editor closes")


def run():
    server = Jmap()

    # 1. Remove writes at once: removing the legacy script, then Cancel
    # instead of OK, leaves it removed on the server.
    editor = open_page()
    loaded(editor)
    select_script(editor, LEGACY_NAME)
    remove = script_button(editor, "Remove")
    wait_for(lambda: remove.getState().contains(pyatspi.STATE_SENSITIVE), "Remove sensitive")
    do_action(remove)
    wait_for(lambda: script_cell(editor, LEGACY_NAME) is None, f"{LEGACY_NAME!r} leaves the list")
    screenshot("legacy-removed")
    close_with_cancel(editor)

    scripts = {script["name"]: script for script in sieve_scripts(server)}
    if LEGACY_NAME in scripts:
        raise ScenarioFailure(
            "Cancel undid a Remove the server had already applied: "
            f"{LEGACY_NAME!r} is still there"
        )
    say(f"server: {LEGACY_NAME!r} stayed removed despite Cancel")

    # 2. Add and Activate write at once: adding a script, filling a rule and
    # activating it, then Cancel instead of OK, leaves it created and active
    # on the server.
    editor = open_page()
    add = loaded(editor)
    click_at(add)
    name_dialog = wait_for(lambda: window("New Filter Script"), "the name dialog")
    click_button(name_dialog, "OK")
    wait_for(lambda: window("New Filter Script") is None, "the name dialog closes")
    wait_for(lambda: script_cell(editor, MANAGED_NAME), f"{MANAGED_NAME!r} listed")
    say(f"added the script {MANAGED_NAME!r}")

    add_rule = rule_button(editor, "Add…")
    wait_for(lambda: add_rule.getState().contains(pyatspi.STATE_SENSITIVE), "rule Add sensitive")
    click_at(add_rule)
    rule_dialog = wait_for(lambda: window("New Rule"), "the rule dialog")
    click_button(rule_dialog, "Add Condition")
    click_button(rule_dialog, "Add Action")

    # In document order: [name, header (hidden), value, address (hidden)].
    entries = list(descendants(rule_dialog, "text"))
    if len(entries) != 4:
        raise ScenarioFailure(f"the rule dialog has {len(entries)} text fields, not 4")
    set_text(entries[0], RULE_NAME)
    set_text(entries[2], "invoice")

    # In document order: [combine, field, op, action1, folder1].
    combos = list(descendants(rule_dialog, "combo box"))
    if len(combos) != 5:
        raise ScenarioFailure(f"the rule dialog has {len(combos)} combo boxes, not 5")
    folders = folder_paths(server)
    invoices_row = next((i for i, (_, path) in enumerate(folders) if path == INVOICES_FOLDER), None)
    if invoices_row is None:
        raise ScenarioFailure(f"the server has no mailbox {INVOICES_FOLDER!r}")
    combos[4].querySelection().selectChild(invoices_row)
    wait_for(
        lambda: selected_item_name(combos[4]) == INVOICES_FOLDER,
        "the folder combo shows Invoices",
    )

    ok = find(rule_dialog, "push button", "OK")
    click_at(ok)
    wait_for(lambda: window("New Rule") is None, "the rule dialog closes")
    wait_for(lambda: find(editor, "table cell", RULE_NAME), f"{RULE_NAME!r} listed in the rules")
    say("added the rule")

    activate = wait_for(
        lambda: find(editor, "push button", "Activate", showing=True), "the Activate button"
    )
    wait_for(lambda: activate.getState().contains(pyatspi.STATE_SENSITIVE), "Activate sensitive")
    do_action(activate)
    wait_for(lambda: status_text(editor) == "", "Activate's own round trip finishes")
    screenshot("rule-activated")
    close_with_cancel(editor)

    scripts_after = {script["name"]: script for script in sieve_scripts(server)}
    managed = scripts_after.get(MANAGED_NAME)
    if managed is None:
        raise ScenarioFailure(
            f"Cancel undid an Add the server had already applied: no {MANAGED_NAME!r}"
        )
    if not managed.get("isActive"):
        raise ScenarioFailure(
            "Cancel undid an Activate the server had already applied: not active"
        )
    say(f"server: {MANAGED_NAME!r} stayed created and active despite Cancel")

    expect_no_errors()


main(setup, run)
