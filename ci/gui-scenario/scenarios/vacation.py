# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: the Vacation Responder page of the account editor.

1. The server has an enabled autoresponder. The page loads it: the check
   box is on, and the subject and the message show the server values.
2. Alice changes the subject and clicks OK. The server has the new subject.
   The other fields keep their values, also the two dates.
3. Alice opens the editor again and clicks OK without a change. The page
   writes nothing: the state of the VacationResponse stays the same.
4. Alice turns the automatic replies off and clicks OK. The server has
   isEnabled false and keeps the subject and the message.
"""

from scenario import (
    Jmap,
    ScenarioFailure,
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
VACATION_CAPABILITY = "urn:ietf:params:jmap:vacationresponse"
EDITOR = "Account Editor"
PAGE = "Vacation Responder"
CHECK_BOX = "Send automatic replies"
FROM_DATE = "2026-11-02T00:00:00Z"
TO_DATE = "2026-11-06T00:00:00Z"
SUBJECT = "Out of office"
NEW_SUBJECT = "Back on Monday"
BODY = "I am away this week."
# The X keysym of the Menu key.
MENU_KEY = 0xFF67


def vacation(server):
    """Return the VacationResponse singleton and the state of the type."""
    result = server.call(
        "VacationResponse/get", {"ids": ["singleton"]}, extra_using=(VACATION_CAPABILITY,)
    )
    return result["list"][0], result["state"]


def setup():
    write_collection_mail_account(ACCOUNT)
    server = Jmap()
    patch = {
        "isEnabled": True,
        "fromDate": FROM_DATE,
        "toDate": TO_DATE,
        "subject": SUBJECT,
        "textBody": BODY,
    }
    updated = server.call(
        "VacationResponse/set",
        {"update": {"singleton": patch}},
        extra_using=(VACATION_CAPABILITY,),
    )
    if "singleton" not in (updated.get("updated") or {}):
        raise ScenarioFailure(f"the mock refused the autoresponder: {updated}")
    say("setup: account source written, the server has an enabled autoresponder")


def open_page():
    """Open the account editor of ACCOUNT on the vacation page and return the editor."""
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
    say("menu Properties of the account")
    editor = wait_for(lambda: window(EDITOR), "the account editor")
    # The editor lists its pages in a sidebar of radio buttons.
    entry = wait_for(lambda: find(editor, "radio button", PAGE), f"the {PAGE!r} sidebar entry")
    do_action(entry)
    wait_for(lambda: is_showing(find(editor, "check box", CHECK_BOX)), f"the {PAGE!r} page")
    say(f"opened the {PAGE!r} page")
    return editor


def loaded(editor):
    """Wait until the page has loaded and return its check box."""
    check_box = find(editor, "check box", CHECK_BOX)
    wait_for(
        lambda: check_box.getState().contains(pyatspi.STATE_SENSITIVE),
        "the check box sensitive (the load is done)",
    )
    return check_box


def close_with_ok(editor):
    click_button(editor, "OK")
    wait_for(lambda: window(EDITOR) is None, "the account editor closes")


def checked(node):
    return node.getState().contains(pyatspi.STATE_CHECKED)


def run():
    server = Jmap()

    # 1. The page loads the autoresponder of the server.
    editor = open_page()
    check_box = loaded(editor)
    if not checked(check_box):
        raise ScenarioFailure("the check box is off, but the autoresponder is enabled")
    subject = wait_for(lambda: text_field(editor, SUBJECT), f"the subject {SUBJECT!r}")
    if text_field(editor, BODY) is None:
        raise ScenarioFailure(f"the page does not show the message {BODY!r}")
    say(f"page: on, subject {SUBJECT!r}, message {BODY!r}")
    screenshot("vacation-loaded")

    # 2. A changed subject goes to the server; the other fields stay.
    set_text(subject, NEW_SUBJECT)
    screenshot("vacation-subject-changed")
    close_with_ok(editor)
    stored, _ = wait_for(
        lambda: (lambda pair: pair if pair[0].get("subject") == NEW_SUBJECT else None)(
            vacation(server)
        ),
        f"the subject {NEW_SUBJECT!r} on the server",
    )
    expected = {
        "isEnabled": True,
        "fromDate": FROM_DATE,
        "toDate": TO_DATE,
        "textBody": BODY,
    }
    for key, value in expected.items():
        if stored.get(key) != value:
            raise ScenarioFailure(f"the save changed {key}: {stored.get(key)!r}, not {value!r}")
    say(f"server: subject {NEW_SUBJECT!r}; enabled, dates and message unchanged")

    # 3. OK without a change writes nothing.
    _, state = vacation(server)
    editor = open_page()
    loaded(editor)
    wait_for(lambda: text_field(editor, NEW_SUBJECT), f"the subject {NEW_SUBJECT!r}")
    close_with_ok(editor)
    after, new_state = vacation(server)
    if new_state != state or after != stored:
        raise ScenarioFailure(f"OK without a change wrote to the server: {after}")
    say("server: OK without a change wrote nothing")

    # 4. Turn the automatic replies off.
    editor = open_page()
    check_box = loaded(editor)
    grid_field = text_field(editor, NEW_SUBJECT)
    do_action(check_box)
    wait_for(lambda: not checked(check_box), "the check box off")
    wait_for(
        lambda: not grid_field.getState().contains(pyatspi.STATE_SENSITIVE),
        "the fields insensitive",
    )
    screenshot("vacation-turned-off")
    close_with_ok(editor)
    stored, _ = wait_for(
        lambda: (lambda pair: pair if not pair[0].get("isEnabled") else None)(vacation(server)),
        "isEnabled false on the server",
    )
    if stored.get("subject") != NEW_SUBJECT or stored.get("textBody") != BODY:
        raise ScenarioFailure(f"turning off lost the text: {stored}")
    say("server: disabled; subject and message kept")
    if any(is_showing(node) for node in descendants(evolution(), "alert", max_depth=2)):
        screenshot("unexpected-alert")
        raise ScenarioFailure("Evolution shows an alert")
    expect_no_errors()


main(setup, run)
