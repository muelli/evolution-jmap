# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: a vCard attached to a mail with LOGO but no PHOTO.

Reading `eab-contact-formatter.c` (3.52.3) shows both `render_title_block`
and `render_compact` fall back to `E_CONTACT_LOGO` when a vCard carries no
`E_CONTACT_PHOTO`, rendering it through the exact same
`id="__evo-contact-photo"` element and `src` URI shape as PHOTO. A vCard
attached to a mail is arbitrary sender bytes, not routed through this
project's own vCard mapping (which drops LOGO on the way out), so a sender
could attach one with only LOGO set. This scenario is
`mail-vcard-photo-fetch.py` with LOGO in place of PHOTO and no PHOTO line at
all, to measure whether Evolution's own, stock default-policy blocking
(item 178) covers this case too.
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
SUBJECT = "A vCard with a logo"
VCARD = (
    "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Helen Httplogo\r\n"
    f"LOGO;VALUE=uri:{cp.PHOTO_URL}\r\nEND:VCARD\r\n"
)
WAIT_SECONDS = 10
SEED_PATH = os.path.join(os.environ.get("GUI_SCENARIO_ARTIFACTS", "."), "mail-vcard-logo-seed.json")


def write_seed():
    """Write the `--seed` JSON for a minimal account with one message: a
    vCard attachment carrying an http(s) LOGO and no PHOTO.

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
        raise ScenarioFailure(f"opening the mail fetched the http logo: {photos.requests}")
    say(f"photo server: no request for the http logo after {WAIT_SECONDS}s")

    click_button(browser, "Toggle View Inline")
    time.sleep(WAIT_SECONDS)
    screenshot("after-expand")
    texts = contacts_shown(browser)
    if not any("Helen Httplogo" in text for text in texts):
        raise ScenarioFailure(f"the compact vCard render never showed: {texts}")
    if photos.requests:
        raise ScenarioFailure(
            f"expanding the vCard fetched the http logo at the default "
            f"image-loading-policy: {photos.requests}"
        )
    say(
        "mail reader: shows the compact vCard inline, still no logo fetch at the "
        "default image-loading-policy"
    )


main(setup, run, mock_args=("--seed", write_seed()))
