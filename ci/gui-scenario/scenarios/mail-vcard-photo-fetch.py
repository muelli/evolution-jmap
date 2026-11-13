# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: what the mail reader does with an http(s) vCard photo attached to a mail.

A mail from someone else carries a vCard attachment whose PHOTO is an http
URL. Item 178 measured (operator, evo-test, stock 3.52.3): Evolution's own
mail reader already treats this photo like any other remote image in a
message. At the default `image-loading-policy` ("never") it stays blocked;
`mail-vcard-photo-fetch-remote-content.py` checks the other half, with the
policy "always", where it loads with the rest of the message's remote
content and needs no click.

jmap-ui's own mail-view placeholder (`external_photo::mail_display_ext`,
`mail_script`) duplicated this stock behaviour and, worse, let a sender
forge its "Load image" button from their own HTML body and make it fetch
an attacker URL as a click tracker (`e_web_view_register_element_clicked`
matches a click by class name across every frame of the view, not only
the vCard's own iframe). Item 178 deleted that placeholder; this scenario
now only confirms Evolution's own, unmodified behaviour.

The attachment's `Content-Disposition` is `attachment` (this seed's own
choice, matching how most senders attach a vCard), so
`e_mail_part_should_show_inline` (`e-mail-part.c`) answers FALSE and the
vcard-inline module's own render stays collapsed behind the attachment
bar's "Toggle View Inline" expander button until a user clicks it: this
scenario does, so it reaches the real inline render, not only the generic
attachment bar.
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
    # real compact render, with the PHOTO URL in a plain `<img src>`
    # (`eab-contact-formatter.c`'s `render_compact`).
    click_button(browser, "Toggle View Inline")
    time.sleep(WAIT_SECONDS)
    screenshot("after-expand")
    texts = contacts_shown(browser)
    if not any("Helen Httpphoto" in text for text in texts):
        raise ScenarioFailure(f"the compact vCard render never showed: {texts}")
    if photos.requests:
        raise ScenarioFailure(
            f"expanding the vCard fetched the http photo at the default "
            f"image-loading-policy: {photos.requests}"
        )
    say(
        "mail reader: shows the compact vCard inline, still no photo fetch at the "
        "default image-loading-policy"
    )


main(setup, run, mock_args=("--seed", write_seed()))
