# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: the Server Filters page of the account editor.

1. The page loads and lists a script the server already holds.
2. Alice adds a script, adds a rule ("Subject contains invoice": move to
   Invoices, mark read), activates it and clicks OK. The server has the
   managed script, active, with the exact Sieve text the rule describes.
3. Alice reopens the page and clicks OK without a change. The managed
   script's blob stays the one from step 2: the save wrote nothing.
4. Alice edits the plain script's text to something invalid and clicks
   Check. The page shows the server's refusal, not "The script is valid.".
5. With the vacation responder on, its script shows as protected: the page
   marks its row and disables the buttons that would change it.
"""

import json
import urllib.request

from scenario import (
    Jmap,
    ScenarioFailure,
    _name,
    click_button,
    descendants,
    do_action,
    evolution,
    expect_no_errors,
    find,
    folder_tree,
    is_showing,
    main,
    pyatspi,
    say,
    screenshot,
    select_folder,
    set_text,
    text_field,
    wait_for,
    window,
    write_collection_mail_account,
)

ACCOUNT = "JMAP scenario mail"
SIEVE_CAPABILITY = "urn:ietf:params:jmap:sieve"
VACATION_CAPABILITY = "urn:ietf:params:jmap:vacationresponse"
EDITOR = "Account Editor"
PAGE = "Server Filters"
# The X keysym of the Menu key.
MENU_KEY = 0xFF67

LEGACY_NAME = "legacy"
LEGACY_TEXT = "keep;\n"
MANAGED_NAME = "evolution-jmap"
RULE_NAME = "Invoices"
INVOICES_FOLDER = "Invoices"
INVALID_TEXT = 'fileinto "Nonexistent";'
SCRIPT_VALID = "The script is valid."
CHECKING = "Checking the script…"


# JMAP helpers. The shared `Jmap` class has no blob upload/download: these
# two go straight to the session's templated URLs (RFC 8620 Sections 6.1
# and 6.2), as `jmap-ui`'s own `filters::io` module does.


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


def folder_paths(server):
    """Return [(id, full path)] of every mailbox, sorted by path.

    Mirrors `jmap-ui`'s own `filters::io::folders`: a mailbox with no
    parent, or a parent the answer does not hold, uses its own name.
    """
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


# AT-SPI helpers specific to this page.


def click_at(node):
    """Click `node` with a synthetic mouse event, not an AT-SPI action call.

    A button whose handler opens a modal dialog (`gtk_dialog_run`) leaves
    the application unreachable over AT-SPI for the rest of an in-flight
    `doAction` call: the action dispatch and the nested main loop it starts
    share the same call stack. A real click does not have this problem, so
    every button that opens a dialog here uses this instead of `do_action`.
    """
    extents = node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
    x = extents.x + extents.width // 2
    y = extents.y + extents.height // 2
    pyatspi.Registry.generateMouseEvent(x, y, "b1c")


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
        lambda: find(editor, "push button", "Add", showing=True), "a showing script Add button"
    )
    wait_for(lambda: add.getState().contains(pyatspi.STATE_SENSITIVE), "the page loaded")
    return add


def structural_nodes(editor):
    """Return (the rules list's button box, the page's status label).

    Both lack a usable AT-SPI name of their own ("Add" and "Remove" name
    both the script list's and the rules list's buttons; the status label's
    text keeps changing). The rules list's "Enabled" column header is
    unique, so this walks up from it: the header's table is the rules
    `GtkTreeView`, its parent is the row that holds the view and its
    buttons, and that row's parent is the page's own content box, whose
    first child is the status label.
    """
    header = wait_for(
        lambda: find(editor, "table column header", "Enabled"), "the 'Enabled' column header"
    )
    table = header.parent
    rules_row = table.parent
    content = rules_row.parent
    return rules_row.getChildAtIndex(1), content.getChildAtIndex(0)


def rule_button(editor, name):
    buttons_box, _ = structural_nodes(editor)
    button = find(buttons_box, "push button", name)
    if button is None:
        raise ScenarioFailure(f"no rule list button named {name!r}")
    return button


def status_text(editor):
    _, status = structural_nodes(editor)
    return _name(status)


def script_cell(editor, text):
    return find(editor, "table cell", text)


def select_script(editor, name):
    cell = wait_for(lambda: script_cell(editor, name), f"{name!r} in the script list")
    cell.queryComponent().grabFocus()


def close_with_ok(editor):
    click_button(editor, "OK")
    wait_for(lambda: window(EDITOR) is None, "the account editor closes")


def close_with_cancel(editor):
    cancel = wait_for(
        lambda: find(editor, "push button", "Cancel", showing=True), "the editor's Cancel button"
    )
    do_action(cancel)
    wait_for(lambda: window(EDITOR) is None, "the account editor closes")


def run():
    server = Jmap()

    # 1. The page loads and lists the seeded script.
    editor = open_page()
    loaded(editor)
    wait_for(lambda: script_cell(editor, LEGACY_NAME), f"{LEGACY_NAME!r} listed")
    screenshot("filters-loaded")
    say(f"page: lists the seeded script {LEGACY_NAME!r}")

    # 2. Alice adds a script, a rule, activates it, and clicks OK.
    add = loaded(editor)
    click_at(add)
    name_dialog = wait_for(lambda: window("New Filter Script"), "the name dialog")
    if text_field(name_dialog, MANAGED_NAME) is None:
        raise ScenarioFailure(f"the suggested script name is not {MANAGED_NAME!r}")
    click_button(name_dialog, "OK")
    wait_for(lambda: window("New Filter Script") is None, "the name dialog closes")
    wait_for(lambda: script_cell(editor, MANAGED_NAME), f"{MANAGED_NAME!r} listed")
    say(f"added the script {MANAGED_NAME!r}")

    add_rule = rule_button(editor, "Add")
    wait_for(lambda: add_rule.getState().contains(pyatspi.STATE_SENSITIVE), "rule Add sensitive")
    click_at(add_rule)
    rule_dialog = wait_for(lambda: window("New Rule"), "the rule dialog")
    click_button(rule_dialog, "Add condition")
    click_button(rule_dialog, "Add action")
    click_button(rule_dialog, "Add action")

    # In document order: [name, header (hidden), value, address, address].
    entries = list(descendants(rule_dialog, "text"))
    if len(entries) != 5:
        raise ScenarioFailure(f"the rule dialog has {len(entries)} text fields, not 5")
    set_text(entries[0], RULE_NAME)
    set_text(entries[2], "invoice")

    # In document order: [combine, field, op, action1, folder1, action2, folder2].
    # `GtkComboBoxText`'s accessible implements AT-SPI's Selection interface
    # directly, so a row's item sets with no popup ever shown.
    combos = list(descendants(rule_dialog, "combo box"))
    if len(combos) != 7:
        raise ScenarioFailure(f"the rule dialog has {len(combos)} combo boxes, not 7")
    folders = folder_paths(server)
    invoices_row = next((i for i, (_, path) in enumerate(folders) if path == INVOICES_FOLDER), None)
    if invoices_row is None:
        raise ScenarioFailure(f"the server has no mailbox {INVOICES_FOLDER!r}")
    invoices_id = folders[invoices_row][0]
    combos[4].querySelection().selectChild(invoices_row)
    wait_for(lambda: combos[4].name == INVOICES_FOLDER, "the folder combo shows Invoices")
    combos[5].querySelection().selectChild(2)  # ActionChoice::MarkRead
    wait_for(lambda: combos[5].name == "Mark as read", "the second action shows Mark as read")
    screenshot("rule-filled")

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
    screenshot("rule-activated")
    close_with_ok(editor)

    scripts = {script["name"]: script for script in sieve_scripts(server)}
    managed = scripts.get(MANAGED_NAME)
    if managed is None:
        raise ScenarioFailure(f"the server has no script named {MANAGED_NAME!r}")
    if not managed.get("isActive"):
        raise ScenarioFailure("the managed script is not active")
    text = download_sieve(server, managed["blobId"])
    rule_json = (
        f'{{"name":"{RULE_NAME}","enabled":true,"combine":"All",'
        '"conditions":[{"field":"Subject","op":"Contains","value":"invoice"}],'
        f'"actions":[{{"MoveTo":{{"id":"{invoices_id}","name":"{INVOICES_FOLDER}"}}}},"MarkRead"],'
        '"stop":false}'
    )
    expected = (
        "# Generated by Evolution (JMAP). Edit it in Evolution's account editor.\n"
        "# evolution-jmap-rules 1\n"
        'require ["fileinto", "imap4flags", "mailboxid"];\n'
        "\n"
        f"# rule {rule_json}\n"
        'if header :contains "subject" "invoice" {\n'
        '    addflag "\\\\Seen";\n'
        f'    fileinto :mailboxid "{invoices_id}" "{INVOICES_FOLDER}";\n'
        "}\n"
    )
    if text != expected:
        raise ScenarioFailure(f"unexpected script text {text!r}, expected {expected!r}")
    say("server: the managed script is active with the exact expected Sieve text")
    blob_before = managed["blobId"]

    # 3. OK without a change writes nothing.
    editor = open_page()
    loaded(editor)
    close_with_ok(editor)
    scripts_after = {script["name"]: script for script in sieve_scripts(server)}
    managed_after = scripts_after.get(MANAGED_NAME)
    if managed_after is None or managed_after["blobId"] != blob_before:
        raise ScenarioFailure(f"OK without a change touched the server: {managed_after}")
    if not managed_after.get("isActive"):
        raise ScenarioFailure("OK without a change deactivated the managed script")
    say("server: OK without a change wrote nothing")

    # 4. An invalid text edit shows the server's refusal after Check.
    editor = open_page()
    loaded(editor)
    select_script(editor, LEGACY_NAME)
    source_view = wait_for(
        lambda: text_field(editor, LEGACY_TEXT), f"the text view showing {LEGACY_TEXT!r}"
    )
    set_text(source_view, INVALID_TEXT)
    check = wait_for(lambda: find(editor, "push button", "Check", showing=True), "Check button")
    do_action(check)
    message = wait_for(
        lambda: (lambda text: text if text not in ("", CHECKING, SCRIPT_VALID) else None)(
            status_text(editor)
        ),
        "a refusal after Check",
    )
    say(f"status after Check: {message!r}")
    screenshot("check-invalid")
    close_with_cancel(editor)
    scripts_after_check = {script["name"]: script for script in sieve_scripts(server)}
    if download_sieve(server, scripts_after_check[LEGACY_NAME]["blobId"]) != LEGACY_TEXT:
        raise ScenarioFailure("Cancel after Check wrote the invalid text to the server")
    say("server: the invalid edit never reached the server")

    # 5. With the vacation responder on, its script shows as protected.
    server.call(
        "VacationResponse/set",
        {
            "update": {
                "singleton": {
                    "isEnabled": True,
                    "fromDate": "2026-11-02T00:00:00Z",
                    "toDate": "2026-11-06T00:00:00Z",
                    "subject": "Out of office",
                    "textBody": "I am away this week.",
                }
            }
        },
        extra_using=(VACATION_CAPABILITY,),
    )
    say("server: the vacation responder is on")

    editor = open_page()
    loaded(editor)
    protected_name = "vacation (used by the Vacation Responder)"
    cell = wait_for(lambda: script_cell(editor, protected_name), f"{protected_name!r} listed")
    cell.queryComponent().grabFocus()
    screenshot("vacation-protected")
    for name in ("Rename", "Remove", "Activate"):
        button = wait_for(lambda name=name: find(editor, "push button", name, showing=True), name)
        wait_for(
            lambda button=button, name=name: not button.getState().contains(
                pyatspi.STATE_SENSITIVE
            ),
            f"{name!r} insensitive for the protected script",
        )
    say("page: the vacation script is protected and its buttons are disabled")
    close_with_cancel(editor)

    if any(is_showing(node) for node in descendants(evolution(), "alert", max_depth=2)):
        screenshot("unexpected-alert")
        raise ScenarioFailure("Evolution shows an alert")
    expect_no_errors()


main(setup, run, mock_args=("--sieve-vacation-script",))
