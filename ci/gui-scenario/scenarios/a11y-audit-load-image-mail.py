# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: the mail reader's Load image placeholder (compact vCard
render) has no unnamed focusable widget and no duplicate Alt+letter
mnemonic.

Item 174(d)'s automated audit (see a11y-audit.py), applied to the second
placeholder item 175 added: `external_photo::mail_display_ext`'s
click-to-load button over the mail view's compact vCard render.
`mail-vcard-photo-fetch.py`'s module-level `main(...)` call runs on
import (no `if __name__ == "__main__"` guard), so this scenario does not
import it; it repeats the small seed and "Toggle View Inline" steps
itself, the same choice `a11y-audit-snooze.py` already made for
`snooze.py`. Scoped to the placeholder's own container, the same
reasoning as `a11y-audit-load-image.py`'s docstring.
"""

import json
import os
import time

from scenario import (
    audit_dialog,
    audit_mnemonics,
    click_button,
    find,
    main,
    menu_item,
    select_folder,
    select_message,
    wait_for,
    window,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
SUBJECT = "A vCard with a photo"
PHOTO_URL = "http://127.0.0.1:8091/photo.png"
VCARD = (
    "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Helen Httpphoto\r\n"
    f"PHOTO;VALUE=uri:{PHOTO_URL}\r\nEND:VCARD\r\n"
)
LOAD_BUTTON = "Load image"
SEED_PATH = os.path.join(os.environ.get("GUI_SCENARIO_ARTIFACTS", "."), "a11y-load-image-mail-seed.json")


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
                            {"name": "contact.vcf", "contentType": "text/vcard", "content": VCARD}
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


def run():
    select_folder(ACCOUNT, "Inbox")
    select_message(SUBJECT)
    menu_item("Message/Open in New Window")
    browser = wait_for(lambda: window(SUBJECT), "the detached message window")
    wait_for(
        lambda: find(browser, "document web", name="Evolution Mail Display"),
        "the message body",
    )
    time.sleep(3)
    click_button(browser, "Toggle View Inline")
    time.sleep(3)
    button = wait_for(
        lambda: find(browser, "push button", LOAD_BUTTON, showing=True),
        "the Load image button",
    )
    placeholder = button.parent
    audit_dialog(placeholder, "Load image placeholder")
    audit_mnemonics(placeholder, "Load image placeholder")


main(setup, run, mock_args=("--seed", write_seed()))
