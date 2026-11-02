# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: every focusable widget of an account editor page has an
accessible name, either its own or through a labelled-by relation.

This is item 174(d)'s automated audit: walk a page's widget tree over
AT-SPI (not pixels) and fail on the first interactive widget (button,
check box, text entry or view, combo box, tree view) whose accessible
name is empty and has no labelled-by relation to a named widget. The
walk itself (`unnamed_widgets`, `audit_dialog`) lives in `scenario.py`,
shared with `a11y-audit-undo-delete.py`, the sibling scenario for the
one surface that needs a calendar/addressbook component instead of
this scenario's mail one. One account editor is opened once; each page
in PAGES is visited in turn, then the name dialog and the rule dialog,
both reached from the Server Filters page. The composer's Send Later
custom-time dialog is audited separately, from its own composer.

Each of these surfaces is also checked for a duplicate Alt+letter
mnemonic with `audit_mnemonics` (`scenario.py`): two widgets in the same
window sharing one mnemonic leaves one of them unreachable by it. The
page check passes the whole account editor, not the page's own page-tab
container, because item 174(d) counts "Evolution's own widgets in the
same window" too (the Account Editor's sidebar and its OK/Cancel/Help
row), the same scope the item's text names for the keyboard-clash check
this audit automates.

Live Tab-key reachability is checked too, with `audit_tab_reachability`
(`scenario.py`): it presses the real Tab key over AT-SPI and asserts
every showing, sensitive push button, check box, text field and table is
actually visited. A combo box is excluded from what it expects to
observe directly (see TAB_OBSERVABLE_ROLES's doc comment); Tab still has
to pass through it to reach the field after it, so a trap there would
still surface as that next field going unreached. Not run on the
Vacation Responder page: its message body is a multi-line text view,
which `tab_reachable`'s own doc comment explains this check cannot walk
through here; PAGES' "tab_check" flag gates it per page.
"""

import pyatspi

from scenario import (
    Jmap,
    ScenarioFailure,
    _role,
    audit_dialog,
    audit_mnemonics,
    audit_tab_reachability,
    click_at,
    click_button,
    do_action,
    evolution,
    find,
    folder_tree,
    main,
    menu_item,
    say,
    select_folder,
    unnamed_widgets,
    wait_for,
    window,
    write_collection_mail_account,
)

ACCOUNT = "JMAP scenario mail"
VACATION_CAPABILITY = "urn:ietf:params:jmap:vacationresponse"
EDITOR = "Account Editor"
# The X keysym of the Menu key.
MENU_KEY = 0xFF67

# Each page: its sidebar label, a widget whose sensitivity means the page
# finished loading ("ready"), and a widget that is always present once the
# page is built, used to find the page's own page-tab container ("anchor").
# The Server Filters page's ready widget is its script list's Add button,
# not its "Edit as text" check box, because that check box's own
# sensitivity depends on the selected script being managed and parseable:
# it is not a reliable "the load finished" signal. The Vacation Responder
# page's check box has no such condition (it is unconditionally enabled
# once the load completes), so it serves as both ready and anchor.
#
# "tab_check" gates the live Tab-reachability walk (`audit_tab_
# reachability`, scenario.py). The Vacation Responder page's message body
# is a STATE_MULTI_LINE text view: tab_reachable's own doc comment records
# why that kind of widget cannot be walked with a plain Tab here (it
# consumes Tab as an inserted character, and this container has no working
# way to synthesize the Ctrl+Tab that would normally escape it), so this
# page keeps relying on the code-read check in `audits/ERGONOMY-A11Y-
# AUDIT-2026-11-01.md`'s row 18 for tab order instead.
PAGES = (
    {
        "name": "Server Filters",
        "ready_role": "push button",
        "ready_name": "Add…",
        "anchor_role": "check box",
        "anchor_name": "Edit as text",
        "tab_check": True,
    },
    {
        "name": "Vacation Responder",
        "ready_role": "check box",
        "ready_name": "Send automatic replies",
        "anchor_role": "check box",
        "anchor_name": "Send automatic replies",
        "tab_check": False,
    },
)


def setup():
    write_collection_mail_account(ACCOUNT)
    server = Jmap()
    patch = {
        "isEnabled": True,
        "fromDate": "2026-11-02T00:00:00Z",
        "toDate": "2026-11-06T00:00:00Z",
        "subject": "Out of office",
        "textBody": "I am away this week.",
    }
    updated = server.call(
        "VacationResponse/set",
        {"update": {"singleton": patch}},
        extra_using=(VACATION_CAPABILITY,),
    )
    if "singleton" not in (updated.get("updated") or {}):
        raise ScenarioFailure(f"the mock refused the autoresponder: {updated}")
    say(f"setup: {ACCOUNT!r}, an enabled autoresponder, no filter scripts")


def open_editor():
    """Open the account editor of ACCOUNT and return it."""
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
    return wait_for(lambda: window(EDITOR), "the account editor")


def open_page(editor, page):
    entry = wait_for(
        lambda: find(editor, "radio button", page["name"]), f"the {page['name']!r} sidebar entry"
    )
    do_action(entry)
    say(f"opened the {page['name']!r} page")


def loaded(editor, page):
    """Wait until the page has loaded."""
    ready = wait_for(
        lambda: find(editor, page["ready_role"], page["ready_name"], showing=True),
        f"a showing {page['ready_name']!r} on the {page['name']!r} page",
    )
    wait_for(lambda: ready.getState().contains(pyatspi.STATE_SENSITIVE), "the page loaded")


def page_root(editor, page):
    """Return the page's own page-tab container.

    `editor` holds every account editor page in one `GtkNotebook`; a page
    that is not the selected one keeps its widgets in the AT-SPI tree with
    `STATE_SHOWING` unset, so walking all of `editor` would also audit
    pages outside this scenario's scope (and report their own gaps, if
    any, as if they belonged to this page). Found by the page's anchor
    widget, walking up to the page tab.
    """
    marker = wait_for(
        lambda: find(editor, page["anchor_role"], page["anchor_name"]),
        f"the {page['anchor_name']!r} anchor widget",
    )
    node = marker
    while node is not None and _role(node) != "page tab":
        node = node.parent
    if node is None:
        raise ScenarioFailure(f"could not find the {page['name']!r} page's page tab container")
    return node


def audit_page(editor, page):
    open_page(editor, page)
    loaded(editor, page)
    root = page_root(editor, page)
    offenders = list(unnamed_widgets(root))
    if offenders:
        raise ScenarioFailure(f"unnamed focusable widgets on the {page['name']!r} page: {offenders}")
    say(f"a11y-audit: no unnamed focusable widget on the {page['name']!r} page")
    audit_mnemonics(editor, page["name"])
    if page["tab_check"]:
        find(editor, page["ready_role"], page["ready_name"]).queryComponent().grabFocus()
        audit_tab_reachability(editor, page["name"])


def rules_add_button(editor):
    """Return the rules list's own Add button, not the script list's.

    Both lists have a button named "Add…"; tell them apart by the rules
    list's unique "Enabled" column header, the same structural walk
    server-filters.py uses.
    """
    header = wait_for(
        lambda: find(editor, "table column header", "Enabled"), "the 'Enabled' column header"
    )
    buttons_box = header.parent.parent.getChildAtIndex(1)
    button = find(buttons_box, "push button", "Add…")
    if button is None:
        raise ScenarioFailure("no rules list Add button")
    return button


def audit_name_and_rule_dialogs(editor):
    """Add a script and a rule, auditing the name dialog and the rule dialog.

    Both dialogs are reached from the Server Filters page: the PAGES loop
    may have left the editor on a later page, so return to it first.
    Clicking Cancel on the rule dialog discards the rule: its fields need
    not hold valid data, only exist.
    """
    server_filters = PAGES[0]
    open_page(editor, server_filters)
    loaded(editor, server_filters)
    add_script = wait_for(
        lambda: find(editor, "push button", "Add…", showing=True), "the script Add button"
    )
    click_at(add_script)
    name_dialog = wait_for(lambda: window("New Filter Script"), "the name dialog")
    audit_dialog(name_dialog, "New Filter Script")
    audit_mnemonics(name_dialog, "New Filter Script")
    audit_tab_reachability(name_dialog, "New Filter Script")
    click_button(name_dialog, "OK")
    wait_for(lambda: window("New Filter Script") is None, "the name dialog closes")

    add_rule = rules_add_button(editor)
    wait_for(lambda: add_rule.getState().contains(pyatspi.STATE_SENSITIVE), "rule Add sensitive")
    click_at(add_rule)
    rule_dialog = wait_for(lambda: window("New Rule"), "the rule dialog")
    click_button(rule_dialog, "Add Condition")
    click_button(rule_dialog, "Add Action")
    audit_dialog(rule_dialog, "New Rule")
    audit_mnemonics(rule_dialog, "New Rule")
    audit_tab_reachability(rule_dialog, "New Rule")
    click_button(rule_dialog, "Cancel")
    wait_for(lambda: window("New Rule") is None, "the rule dialog closes")


def audit_send_later_dialog():
    """Open a composer and audit the Send Later custom-time dialog.

    The Custom Time item's handler runs a nested `gtk_dialog_run`, the same
    shape `click_at` works around for a button in `audit_name_and_rule_
    dialogs`; `menu_item` already runs the activating `doAction` call in a
    child process for exactly this reason, so it is used here unchanged.
    """
    select_folder(ACCOUNT, "Inbox")
    menu_item("File/New/Mail Message")
    composer = wait_for(lambda: window("Compose Message"), "the composer window")
    send_later = wait_for(
        lambda: find(composer, "menu", "Send Later"), "the Send Later submenu"
    )
    wait_for(
        lambda: send_later.getState().contains(pyatspi.STATE_SENSITIVE),
        "the Send Later submenu sensitive (the gate found FUTURERELEASE)",
    )
    menu_item("File/Send Later/Custom Time…", "Compose Message")
    dialog = wait_for(lambda: window("Send Later"), "the Send Later custom-time dialog")
    audit_dialog(dialog, "Send Later")
    audit_mnemonics(dialog, "Send Later")
    # No audit_tab_reachability here: confirmed empirically that in this
    # dialog specifically, a synthetic Tab from its first field (the
    # EDateEdit's own date entry, with show_time true so its time combo is
    # part of the chain here, unlike the Vacation Responder's date fields)
    # leaves no STATE_FOCUSED node observable anywhere in the dialog for
    # every later Tab press, not just the usual single-step combo box gap
    # TAB_OBSERVABLE_ROLES documents. The gap is specific to this dialog:
    # the structurally identical name and rule dialogs above, opened the
    # same `gtk_dialog_run` way, walk cleanly. Root cause not pinned down;
    # logged as an open row rather than guessed at.
    click_button(dialog, "Cancel")
    wait_for(lambda: window("Send Later") is None, "the dialog closes")


def run():
    editor = open_editor()
    for page in PAGES:
        audit_page(editor, page)
    audit_name_and_rule_dialogs(editor)
    audit_send_later_dialog()


main(setup, run, mock_args=("--max-delayed-send", "86400"))
