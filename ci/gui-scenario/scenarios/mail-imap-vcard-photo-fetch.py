# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: the vCard-photo comparison leg of item 175, a plain IMAP account.

`mail-vcard-photo-fetch.py` confirms that Evolution never auto-fetches an
http(s) vCard PHOTO attached to a JMAP mail. Item 175 asks for the same
check against a plain IMAP account, so the finding is known to be
Evolution's own mail-reader behaviour, not something specific to this
project's JMAP backend. This scenario is the IMAP twin: same vCard, same
photo server, same two checks (no fetch behind the attachment bar, no
fetch after "Toggle View Inline"), against `ci/dovecot-oracle.sh`'s
Maildir instead of `jmap-mockd`.

`ci/gui-scenario/run.sh` delivers `../fixtures/imap-vcard-photo.eml` on the
host before this scenario's container starts (the container has no podman
of its own to reach the oracle's sibling container); the oracle itself
must already be running (`ci/dovecot-oracle.sh start`).
"""

import time

import contact_photo as cp
from scenario import (
    ScenarioFailure,
    click_button,
    descendants,
    find,
    main,
    menu_item,
    say,
    select_folder,
    select_message,
    text_of,
    wait_for,
    window,
    write_standalone_imap_account,
)

ACCOUNT = "IMAP scenario mail"
SUBJECT = "A vCard with a photo"
WAIT_SECONDS = 10


def setup():
    write_standalone_imap_account(ACCOUNT)


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

    # `select_folder` dismisses the mail authentication request itself (see
    # mail-imap-oracle-smoke.py): the oracle accepts any password, including
    # the empty one submitted here.
    select_folder(ACCOUNT, "Inbox")
    select_message(SUBJECT)
    menu_item("Message/Open in New Window")
    browser = wait_for(lambda: window(SUBJECT), "the detached message window")

    wait_for(
        lambda: find(browser, "document web", name="Evolution Mail Display"),
        "the message body",
    )
    time.sleep(WAIT_SECONDS)

    texts = contacts_shown(browser)
    if not any("contact.vcf" in text for text in texts):
        raise ScenarioFailure(f"the attachment never showed as a real vCard part: {texts}")
    if photos.requests:
        raise ScenarioFailure(f"opening the IMAP mail fetched the http photo: {photos.requests}")
    say("IMAP mail reader: shows the vCard as a real attachment, no photo fetch")

    click_button(browser, "Toggle View Inline")
    time.sleep(WAIT_SECONDS)
    texts = contacts_shown(browser)
    if not any("Helen Httpphoto" in text for text in texts):
        raise ScenarioFailure(f"the compact vCard render never showed: {texts}")
    if photos.requests:
        raise ScenarioFailure(
            f"expanding the vCard fetched the http photo over a plain IMAP "
            f"account too: {photos.requests}"
        )
    say("IMAP mail reader: shows the compact vCard inline, still no photo fetch")


main(setup, run)
