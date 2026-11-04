# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: what the mail reader does with an http(s) vCard photo attached to a mail.

A mail from someone else carries a vCard attachment whose PHOTO is an http
URL. This scenario measures first, as item 175 requires: does opening the
mail make Evolution fetch the photo URL on its own.

The attachment's `Content-Disposition` is `attachment` (this seed's own
choice, matching how most senders attach a vCard), so
`e_mail_part_should_show_inline` (`e-mail-part.c`) answers FALSE and the
vcard-inline module's own render stays collapsed behind the attachment
bar's "Toggle View Inline" expander button until a user clicks it: this
scenario does, so it reaches the real inline render, not only the generic
attachment bar. The compact render writes a plain
`<img src="the-photo-url">` (`eab-contact-formatter.c`'s `render_compact`),
unlike a `file://` photo, yet no fetch happens at Evolution's default
remote image policy (`image-loading-policy` "never"). See the harness log
entry this scenario's commit is paired with for the full trail.

Past the expand, this scenario also drives jmap-ui's click-to-load
placeholder (`external_photo::mail_display_ext`): the "Load image" button
it puts in place of the `<img>` the compact render would otherwise show,
same as `contact-photo-fetch.py` already does for the standalone address
book preview.

Not yet covered here: item 175 also asks to check with the user's "Load
remote content" setting turned on. Switching `image-loading-policy` to
"always" live, while this message is open, makes Evolution reformat the
message on its own (a new "Load Images" information bar appears and the
vCard section collapses back to its unexpanded state), so this needs its
own, carefully observed follow-up rather than a quick reuse of this
window; left open, see the harness log.
"""

import json
import os
import time
import urllib.request

import contact_photo as cp
from scenario import (
    ScenarioFailure,
    click_button,
    descendants,
    find,
    main,
    menu_item,
    say,
    screenshot,
    select_folder,
    select_message,
    text_of,
    wait_for,
    window,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
SUBJECT = "A vCard with a photo"
VCARD = (
    "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Helen Httpphoto\r\n"
    f"PHOTO;VALUE=uri:{cp.PHOTO_URL}\r\nEND:VCARD\r\n"
)
WAIT_SECONDS = 10
# The label of the placeholder and the text of its button, as jmap-ui writes
# them (same English text as the address book preview's own placeholder).
PLACEHOLDER = "Externally hosted image"
LOAD_BUTTON = "Load image"
SEED_PATH = os.path.join(os.environ.get("GUI_SCENARIO_ARTIFACTS", "."), "mail-vcard-seed.json")


def write_seed():
    """Write the `--seed` JSON for a minimal account with one message: a
    vCard attachment carrying an http(s) PHOTO.

    Returns the path, so the caller can pass it straight to `--seed`.
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
                        "textBody": "See the attached contact.",
                        "receivedAt": "2026-01-02T03:04:05Z",
                        "attachments": [
                            {
                                "name": "contact.vcf",
                                "contentType": "text/vcard",
                                "content": VCARD,
                            }
                        ],
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
    say("setup: account sources written, seed file holds the vCard message")


def contacts_shown(browser):
    """The text nodes of `browser`, for an assertion on the rendered content."""
    texts = []
    for node in descendants(browser):
        try:
            texts.append(text_of(node))
        except Exception:  # noqa: BLE001
            continue
    return texts


def run():
    photos = cp.PhotoServer()

    with urllib.request.urlopen(cp.PHOTO_URL, timeout=5) as response:
        if response.status != 200:
            raise ScenarioFailure(f"the photo server answered {response.status}")
    if photos.requests != ["/photo.png"]:
        raise ScenarioFailure(f"the photo server is not reachable: {photos.requests}")
    photos.requests.clear()
    say("photo server: reachable directly, request log cleared")

    select_folder(ACCOUNT, "Inbox")
    select_message(SUBJECT)
    menu_item("Message/Open in New Window")
    browser = wait_for(lambda: window(SUBJECT), "the detached message window")
    screenshot("detached-window")

    # The inline reading pane of the 3-pane view does not reliably realise
    # its web view in this harness (confirmed against snooze-preview.py's own
    # mock log: no body fetch either, even after a wait). A detached message
    # window does, the same way snooze-preview.py already relies on it.
    wait_for(
        lambda: find(browser, "document web", name="Evolution Mail Display"),
        "the message body",
    )
    time.sleep(WAIT_SECONDS)
    screenshot("after-wait")

    texts = contacts_shown(browser)
    if not any("contact.vcf" in text for text in texts):
        raise ScenarioFailure(f"the attachment never showed as a real vCard part: {texts}")
    say("mail reader: shows the vCard as a real attachment, not a fetch artifact")

    if photos.requests:
        raise ScenarioFailure(f"opening the mail fetched the http photo: {photos.requests}")
    say(f"photo server: no request for the http photo after {WAIT_SECONDS}s")

    # The attachment bar's own expander reveals the vcard-inline module's
    # real compact render (see the module docstring for why a click is
    # needed at all). Only past this click does the scenario reach the
    # `<img src="the-photo-url">` item 175 is actually worried about.
    click_button(browser, "Toggle View Inline")
    time.sleep(WAIT_SECONDS)
    screenshot("after-expand")
    texts = contacts_shown(browser)
    if not any("Helen Httpphoto" in text for text in texts):
        raise ScenarioFailure(f"the compact vCard render never showed: {texts}")
    if photos.requests:
        raise ScenarioFailure(f"expanding the vCard fetched the http photo: {photos.requests}")
    say("mail reader: shows the compact vCard inline, still no photo fetch")

    if not any(PLACEHOLDER in text for text in texts):
        raise ScenarioFailure(f"the placeholder never showed: {texts}")
    if not any(cp.PHOTO_URL in text for text in texts):
        raise ScenarioFailure(f"the placeholder does not name the URL: {texts}")
    say(f"mail reader: the placeholder names {cp.PHOTO_URL}")

    click_button(browser, LOAD_BUTTON)
    wait_for(lambda: photos.requests or None, "the photo server to see the click's request")
    time.sleep(2)
    screenshot("after-load-click")
    if photos.requests != ["/photo.png"]:
        raise ScenarioFailure(f"the click made an unexpected request shape: {photos.requests}")
    say(f"photo server: exactly one request after the click: {photos.requests}")


main(setup, run, mock_args=("--seed", write_seed()))
