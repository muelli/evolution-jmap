# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: every focusable widget of the Server Filters page has an
accessible name, either its own or through a labelled-by relation.

This is the first slice of item 174(d)'s automated audit: walk the page's
widget tree over AT-SPI (not pixels) and fail on the first interactive
widget (button, check box, text entry or view, combo box, tree view) whose
accessible name is empty and has no labelled-by relation to a named widget.
Other surfaces, duplicate-mnemonic detection and live Tab-key reachability
follow in later scenarios or slices of this one.
"""

import pyatspi

from scenario import (
    ScenarioFailure,
    _name,
    _role,
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
EDITOR = "Account Editor"
PAGE = "Server Filters"
# The X keysym of the Menu key.
MENU_KEY = 0xFF67

# Roles a screen reader can land keyboard focus on. Labels, static text and
# plain containers are excluded: they are not focusable themselves. A
# GtkTreeView reports as "table" over AT-SPI (confirmed in a dump of this
# very page), not "tree table".
FOCUSABLE_ROLES = {"push button", "check box", "text", "combo box", "table"}


def setup():
    write_collection_mail_account(ACCOUNT)
    say(f"setup: {ACCOUNT!r} with no filter scripts")


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
    """Wait until the page has loaded."""
    add = wait_for(
        lambda: find(editor, "push button", "Add…", showing=True), "a showing script Add button"
    )
    wait_for(lambda: add.getState().contains(pyatspi.STATE_SENSITIVE), "the page loaded")


def page_root(editor):
    """Return the Server Filters page's own page-tab container.

    `editor` holds every account editor page in one `GtkNotebook`; a page
    that is not the selected one keeps its widgets in the AT-SPI tree with
    `STATE_SHOWING` unset, so walking all of `editor` would also audit
    pages outside this scenario's scope (and report their own gaps, if
    any, as if they belonged to Server Filters). Found by its "Edit as
    text" check box, the one widget unique to this page.
    """
    marker = wait_for(
        lambda: find(editor, "check box", "Edit as text"), "the 'Edit as text' check box"
    )
    node = marker
    while node is not None and _role(node) != "page tab":
        node = node.parent
    if node is None:
        raise ScenarioFailure("could not find the Server Filters page's page tab container")
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
    accessible name and no labelled-by relation to a named widget."""
    for node in descendants(root):
        if _role(node) not in FOCUSABLE_ROLES:
            continue
        if _name(node) or labelled_by_name(node):
            continue
        yield _role(node)


def run():
    editor = open_page()
    loaded(editor)
    root = page_root(editor)
    offenders = list(unnamed_widgets(root))
    if offenders:
        raise ScenarioFailure(f"unnamed focusable widgets on the {PAGE!r} page: {offenders}")
    say(f"a11y-audit: no unnamed focusable widget on the {PAGE!r} page")


main(setup, run)
