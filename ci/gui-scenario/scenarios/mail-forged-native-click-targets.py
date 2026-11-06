# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: a mail body forges Evolution's own native mail click targets.

Item 178's design note points out why jmap-ui's own click mechanism keys a
button's `value` to an unguessable per-render token: Evolution's native mail
click handlers (`e_web_view_register_element_clicked`) match a click by
class name across every frame of the view, with no check that the element
came from the part it claims to belong to. A sender's HTML body can put an
element of any registered class anywhere on the page. This scenario builds
such a body and records, for each native target it covers, whether a forged
element reaches real behaviour.

`attachment-expander` and `attachment-menu` (`mail/e-mail-display.c`,
registered on every message) look the clicked element's `value` up against
the live in-memory addresses of the message's own `EAttachment` objects
(`mail_display_ref_attachment_from_element`), formatted as a bare `%p`. A
forged value a sender writes into a static message can only guess at an
address that exists in a future reader's process; this scenario checks that
a plausible-looking but wrong one changes nothing and opens no menu.

`__evo-remote-content-img`'s handler (`mail_display_remote_content_clicked_cb`)
reads none of the clicked element's id, class or value: it only emits a
signal that shows the same remote-content popover a real indicator would. A
forged element of this class can make the popover appear unprompted, but
carries no attacker data into it.

Not covered here, each left for a follow-up claim:

`secure-button` and `itip-button` are only registered when the message
carries a real signed part or a real calendar invitation
(`em-format/e-mail-part-secure-button.c`, `modules/itip-formatter/
itip-view.c`), so a forged element of either class in a message with
neither does nothing at all, for lack of any listener. Both also gate on a
`value` prefixed with `g_strdup_printf("%p:", ...)` of an in-memory object a
static message cannot predict, the same shape as attachment-expander above.
Building a live signed or iTIP fixture to exercise the registered case is
its own piece of work.

`org-gnome-vcard-save-button` (`modules/vcard-inline/e-mail-formatter-
vcard.c`) checks the clicked value with `strstr(part_id, element_value)`,
true for an empty `value`, so the real button's own `value` is the plain
`part_id` string, not a secret pointer; a forged copy with no `value`
attribute at all would pass that check anywhere on the page. This was not
reproduced here: a debug probe clicking the message's own real "Save To
Addressbook" button through `click_at` (the synthetic-mouse path the
in-flight `gtk_dialog_run` requires, since the click reaches it through a
WebKit accessible node) also failed to open the dialog, so the gap is in
this harness's click delivery for that specific control, not evidence
either way about the forged case. Needs a different interaction path
(for example driving the click through `webkit_web_view_run_javascript`,
or a real X11 click through the compositor) before it can be measured.
"""

import json
import os
import time

from scenario import (
    ScenarioFailure,
    do_action,
    evolution,
    find,
    main,
    menu_item,
    say,
    screenshot,
    select_folder,
    select_message,
    wait_find,
    wait_for,
    window,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
SUBJECT = "Forged native click targets"
WAIT_SECONDS = 10

EXPANDER_TEXT = "Forged Attachment Expander"
MENU_TEXT = "Forged Attachment Menu"
REMOTE_TEXT = "Forged Remote Content"

# A plausible-looking but wrong value: the shape of a real pointer
# (`g_strdup_printf("%p", ...)`), never one this process actually allocated.
FAKE_POINTER = "0x55fdead0beef"

# attachment-expander, attachment-menu and __evo-remote-content-img are all
# registered for the top-level document on every message
# (`mail/e-mail-display.c`'s own `content_loaded`), so a plain HTML body is
# enough to reach every one of them.
HTML_BODY = (
    "<p>Forged native click targets test (item 178).</p>"
    f'<button type="button" class="attachment-expander" '
    f'value="{FAKE_POINTER}">{EXPANDER_TEXT}</button>'
    f'<button type="button" class="attachment-menu" '
    f'value="{FAKE_POINTER}">{MENU_TEXT}</button>'
    f'<button type="button" class="__evo-remote-content-img" '
    f'value="">{REMOTE_TEXT}</button>'
)

SEED_PATH = os.path.join(
    os.environ.get("GUI_SCENARIO_ARTIFACTS", "."), "mail-forged-native-seed.json"
)


def write_seed():
    """Write the `--seed` JSON: one HTML message forging three native click
    targets in its body.
    """
    seed = {
        "accounts": [
            {
                "id": "A1",
                "name": "alice@example.com",
                "mailboxes": [
                    {"id": "M1", "name": "Inbox", "role": "inbox"},
                    {"id": "M2", "name": "Sent", "role": "sent"},
                    {"id": "M3", "name": "Drafts", "role": "drafts"},
                ],
                "emails": [
                    {
                        "mailboxId": "M1",
                        "from": {"name": "Sender", "email": "sender@example.test"},
                        "to": [{"name": "Alice", "email": "alice@example.com"}],
                        "subject": SUBJECT,
                        "textBody": HTML_BODY,
                        "bodyIsHtml": True,
                        "receivedAt": "2026-01-02T03:04:05Z",
                    }
                ],
            }
        ]
    }
    with open(SEED_PATH, "w", encoding="utf-8") as out:
        json.dump(seed, out)
    return SEED_PATH


def setup():
    write_standalone_mail_account(ACCOUNT)
    say("setup: account sources written, seed file holds the forged message")


def top_level_window_count():
    return evolution().childCount


def run():
    select_folder(ACCOUNT, "Inbox")
    select_message(SUBJECT)
    menu_item("Message/Open in New Window")
    browser = wait_for(lambda: window(SUBJECT), "the detached message window")
    time.sleep(WAIT_SECONDS)
    screenshot("forged-mail-open")

    for expected in (EXPANDER_TEXT, MENU_TEXT, REMOTE_TEXT):
        if find(browser, "push button", expected, showing=True) is None:
            raise ScenarioFailure(f"the forged button {expected!r} never rendered")
    say("mail reader: the forged buttons render")

    # attachment-expander: a wrong-but-plausible value matches no live
    # EAttachment, so the handler's own lookup fails and nothing happens.
    # Safe to click through AT-SPI's doAction: the handler never opens a
    # nested loop when the lookup fails.
    windows_before = top_level_window_count()
    expander = wait_find(None, EXPANDER_TEXT, showing=True)
    do_action(expander)
    time.sleep(2)
    screenshot("after-forged-expander")
    if top_level_window_count() != windows_before:
        raise ScenarioFailure("a forged attachment-expander click opened a window")
    say("attachment-expander: forged click with a wrong value does nothing")

    # attachment-menu: same lookup, so no popup menu appears.
    menu_button = wait_find(None, MENU_TEXT, showing=True)
    do_action(menu_button)
    time.sleep(2)
    screenshot("after-forged-menu")
    if top_level_window_count() != windows_before:
        raise ScenarioFailure("a forged attachment-menu click opened a window")
    say("attachment-menu: forged click with a wrong value opens no menu")

    # __evo-remote-content-img: the handler reads none of the clicked
    # element's data, only the click position, so this is safe to fire
    # through doAction too.
    remote = wait_find(None, REMOTE_TEXT, showing=True)
    do_action(remote)
    time.sleep(2)
    screenshot("after-forged-remote-content")
    say("__evo-remote-content-img: forged click carries no attacker data")


main(setup, run, mock_args=("--seed", write_seed()))
