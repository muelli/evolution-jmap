# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: the sender-URL vCard photo with "Load remote content" already on.

`mail-vcard-photo-fetch.py` checks the default `image-loading-policy`
("never"): the vCard photo stays blocked. This scenario sets the policy to
"always" before Evolution starts (flipping it live while a message is open
reformats the message instead, which is not a clean measurement) and checks
the photo stays blocked even then.

Measured 2026-11-05 against this container's stock 3.52.3, both at "never"
and at "always": the vCard photo never loads either way. Reading
`e-mail-formatter-vcard.c` and `eab-contact-formatter.c` explains why this
differs from an ordinary `<img>` in a mail body: the compact vCard render is
its own `mail:`-scheme iframe, and inside it `eab-contact-formatter.c`
writes the PHOTO/LOGO URL straight into `<img src="http(s)://...">`.
Evolution's own remote-content gating (`e-mail-display.c`,
`mail_display_uri_requested_cb`) only ever applies to a request already
rewritten to the `evo-http(s):` scheme; a body `<img>` gets that rewrite
from the formatter, but this plain `http(s):` one does not, and WebKit
itself never starts a plain network request for an `http(s):` resource of a
page loaded from the privileged `mail:` scheme. So the vCard photo is
unconditionally blocked, by accident of this scheme mismatch, not by
`image-loading-policy`: this scenario's own request log staying empty with
the policy "always" is the expected, correct result, not a gap.

This is `mail-vcard-photo-fetch.py` unchanged apart from this file's own
docstring and the final assertions; not imported from it, the same choice
`mail-vcard-logo-fetch.py` already made, since `main()` below runs on
import with no `__main__` guard.
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
SUBJECT = "A vCard with a photo, remote content on"
VCARD = (
    "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Helen Httpphoto\r\n"
    f"PHOTO;VALUE=uri:{cp.PHOTO_URL}\r\nEND:VCARD\r\n"
)
WAIT_SECONDS = 10
SEED_PATH = os.path.join(
    os.environ.get("GUI_SCENARIO_ARTIFACTS", "."), "mail-vcard-remote-content-seed.json"
)


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

    wait_for(
        lambda: find(browser, "document web", name="Evolution Mail Display"),
        "the message body",
    )
    time.sleep(WAIT_SECONDS)
    screenshot("after-wait")

    texts = contacts_shown(browser)
    if not any("contact.vcf" in text for text in texts):
        raise ScenarioFailure(f"the attachment never showed as a real vCard part: {texts}")
    if photos.requests:
        raise ScenarioFailure(
            f"opening the mail fetched the http photo before the vCard even showed: "
            f"{photos.requests}"
        )
    say(f"photo server: no request for the http photo after {WAIT_SECONDS}s, before any expand")

    # The attachment bar's own expander reveals the vcard-inline module's
    # real compact render: a plain `<img src="http(s)://...">`, never
    # rewritten to `evo-http(s):`, so Evolution's own remote-content gating
    # never sees it and the policy "always" changes nothing (see the module
    # docstring).
    click_button(browser, "Toggle View Inline")
    time.sleep(WAIT_SECONDS)
    screenshot("after-expand")
    texts = contacts_shown(browser)
    if not any("Helen Httpphoto" in text for text in texts):
        raise ScenarioFailure(f"the compact vCard render never showed: {texts}")
    if photos.requests:
        raise ScenarioFailure(
            f"expanding the vCard fetched the http photo with image-loading-policy "
            f"'always': {photos.requests}"
        )
    say(
        "mail reader: shows the compact vCard inline, still no photo fetch even with "
        "image-loading-policy 'always'"
    )


main(setup, run, mock_args=("--seed", write_seed()))
