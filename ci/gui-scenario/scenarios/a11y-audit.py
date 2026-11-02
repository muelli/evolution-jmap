# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: every focusable widget of an account editor page has an
accessible name, either its own or through a labelled-by relation.

This is item 174(d)'s automated audit: walk a page's widget tree over
AT-SPI (not pixels) and fail on the first interactive widget (button,
check box, text entry or view, combo box, tree view) whose accessible
name is empty and has no labelled-by relation to a named widget. One
account editor is opened once; each page in PAGES is visited in turn,
then the name dialog and the rule dialog, both reached from the Server
Filters page. Duplicate-mnemonic detection and live Tab-key
reachability, and every surface not yet covered, follow in later
slices.
"""

import pyatspi

from scenario import (
    Jmap,
    ScenarioFailure,
    _name,
    _role,
    click_at,
    click_button,
    descendants,
    do_action,
    evolution,
    find,
    folder_tree,
    main,
    say,
    select_folder,
    wait_for,
    window,
    write_collection_mail_account,
)

ACCOUNT = "JMAP scenario mail"
VACATION_CAPABILITY = "urn:ietf:params:jmap:vacationresponse"
EDITOR = "Account Editor"
# The X keysym of the Menu key.
MENU_KEY = 0xFF67

# Roles a screen reader can land keyboard focus on. Labels, static text and
# plain containers are excluded: they are not focusable themselves. A
# GtkTreeView reports as "table" over AT-SPI (confirmed in a dump of the
# Server Filters page), not "tree table".
FOCUSABLE_ROLES = {"push button", "check box", "text", "combo box", "table"}

# Each page: its sidebar label, a widget whose sensitivity means the page
# finished loading ("ready"), and a widget that is always present once the
# page is built, used to find the page's own page-tab container ("anchor").
# The Server Filters page's ready widget is its script list's Add button,
# not its "Edit as text" check box, because that check box's own
# sensitivity depends on the selected script being managed and parseable:
# it is not a reliable "the load finished" signal. The Vacation Responder
# page's check box has no such condition (it is unconditionally enabled
# once the load completes), so it serves as both ready and anchor.
PAGES = (
    {
        "name": "Server Filters",
        "ready_role": "push button",
        "ready_name": "Add…",
        "anchor_role": "check box",
        "anchor_name": "Edit as text",
    },
    {
        "name": "Vacation Responder",
        "ready_role": "check box",
        "ready_name": "Send automatic replies",
        "anchor_role": "check box",
        "anchor_name": "Send automatic replies",
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


def labelled_by_name(node):
    """Return the name of the first labelled-by relation target, or "".

    GTK sets this relation automatically for a field whose caption used
    `gtk_label_set_mnemonic_widget`: a screen reader resolves the field's
    name through it even though the field's own accessible name stays
    empty, so this counts as named too.
    """
    try:
        relations = node.getRelationSet()
    except Exception:  # noqa: BLE001
        return ""
    for relation in relations:
        if relation.getRelationType() != pyatspi.RELATION_LABELLED_BY:
            continue
        for i in range(relation.getNTargets()):
            text = _name(relation.getTarget(i))
            if text:
                return text
    return ""


def unnamed_widgets(root):
    """Yield the role of every focusable descendant of `root` with no
    accessible name and no labelled-by relation to a named widget.

    Skips a node that is not `STATE_SHOWING`: an `EDateEdit` with
    `show_time` off keeps its hidden time combo (and that combo's own
    editable entry) in the AT-SPI tree, state VISIBLE but not SHOWING, the
    same way a notebook page that is not the selected one keeps its
    widgets in the tree with SHOWING unset. Tab can never reach a hidden
    widget, so it is not a reachable "focusable widget with no name" in
    the sense this scenario checks.
    """
    for node in descendants(root):
        if _role(node) not in FOCUSABLE_ROLES:
            continue
        if not node.getState().contains(pyatspi.STATE_SHOWING):
            continue
        if _name(node) or labelled_by_name(node):
            continue
        yield _role(node)


def audit_page(editor, page):
    open_page(editor, page)
    loaded(editor, page)
    root = page_root(editor, page)
    offenders = list(unnamed_widgets(root))
    if offenders:
        raise ScenarioFailure(f"unnamed focusable widgets on the {page['name']!r} page: {offenders}")
    say(f"a11y-audit: no unnamed focusable widget on the {page['name']!r} page")


def audit_dialog(dialog, label):
    offenders = list(unnamed_widgets(dialog))
    if offenders:
        raise ScenarioFailure(f"unnamed focusable widgets in the {label!r} dialog: {offenders}")
    say(f"a11y-audit: no unnamed focusable widget in the {label!r} dialog")


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
    click_button(name_dialog, "OK")
    wait_for(lambda: window("New Filter Script") is None, "the name dialog closes")

    add_rule = rules_add_button(editor)
    wait_for(lambda: add_rule.getState().contains(pyatspi.STATE_SENSITIVE), "rule Add sensitive")
    click_at(add_rule)
    rule_dialog = wait_for(lambda: window("New Rule"), "the rule dialog")
    click_button(rule_dialog, "Add Condition")
    click_button(rule_dialog, "Add Action")
    audit_dialog(rule_dialog, "New Rule")
    click_button(rule_dialog, "Cancel")
    wait_for(lambda: window("New Rule") is None, "the rule dialog closes")


def run():
    editor = open_editor()
    for page in PAGES:
        audit_page(editor, page)
    audit_name_and_rule_dialogs(editor)


main(setup, run)
