# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: the sender-URL vCard photo with "Load remote content" already on.

Item 175 also asks to measure with the user's "Load remote content"
setting ("Always load images from the Internet" in Evolution's own Mail
Preferences, GSettings `org.gnome.evolution.mail image-loading-policy
always`) already on, not just at the "never" default
`mail-vcard-photo-fetch.py` measures. `ci/gui-scenario/run.sh` sets the
policy for this scenario's name before Evolution starts, so the message is
never open while the policy changes: that avoided case, confirmed in
`mail-vcard-photo-fetch.py`'s own docstring, reformats the message on its
own (a "Load Images" information bar appears, the vCard section collapses
back to its unexpanded state) and so is not a clean measurement.

Reading Evolution's own `e-mail-display.c` (`mail_display_uri_requested_cb`,
3.52.3 source) explains why this case matters: every `http(s)` resource a
message's WebView asks for, from any formatter, goes through one signal
handler that blocks the request only when `image_loading_policy ==
E_IMAGE_LOADING_POLICY_NEVER` and the host has no per-site or per-sender
allowance. With the policy not "never", that block never triggers, so
Evolution redirects the request to its own `evo-http(s)` scheme and fetches
it, the same path an ordinary `<img>` in an HTML mail body uses. The
placeholder script (`external_photo::script`) only hides the `<img>` with
CSS and reads its `src` back out for the button; it does not clear or
rewrite `src`, so it does not, on its own, stop a fetch that this
WebView-level policy already allowed. This scenario measures what actually
happens: either this is consistent with Evolution's own remote-content
behaviour for any image in a mail (the user opted into always loading
remote content, so a vCard photo loading like any other embedded image is
not a gap), or it shows the placeholder gives no protection once the user
has turned the general setting on, which is a finding to log either way.

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
PLACEHOLDER = "Externally hosted image"
LOAD_BUTTON = "Load image"
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
            f"opening the mail fetched the http photo, with the policy on 'always': "
            f"{photos.requests}"
        )
    say(f"photo server: no request for the http photo after {WAIT_SECONDS}s, before any expand")

    click_button(browser, "Toggle View Inline")
    time.sleep(WAIT_SECONDS)
    screenshot("after-expand")
    texts = contacts_shown(browser)
    if not any("Helen Httpphoto" in text for text in texts):
        raise ScenarioFailure(f"the compact vCard render never showed: {texts}")

    placeholder_shown = any(PLACEHOLDER in text for text in texts)
    say(
        f"image-loading-policy always: placeholder shown={placeholder_shown}, "
        f"requests after expand={photos.requests}"
    )
    if photos.requests:
        raise ScenarioFailure(
            "the photo loaded on its own with image-loading-policy 'always': "
            f"{photos.requests}. The placeholder's CSS-only hide does not stop "
            "Evolution's own uri-requested handler from fetching it once the "
            "policy is not 'never'; item 175's own rule is that this photo must "
            "never load without a click, independent of the general setting, so "
            "this is a real gap."
        )
    say("photo server: no request after the expand either, even with the policy on 'always'")

    if not placeholder_shown:
        raise ScenarioFailure(f"the placeholder did not show with the policy on 'always': {texts}")
    if not any(cp.PHOTO_URL in text for text in texts):
        raise ScenarioFailure(f"the placeholder does not name the URL: {texts}")
    say(f"mail reader: the placeholder still shows and still names {cp.PHOTO_URL}")

    click_button(browser, LOAD_BUTTON)
    wait_for(lambda: photos.requests or None, "the photo server to see the click's request")
    time.sleep(2)
    screenshot("after-load-click")
    if photos.requests != ["/photo.png"]:
        raise ScenarioFailure(f"the click made an unexpected request shape: {photos.requests}")
    say(f"photo server: exactly one request after the click: {photos.requests}")


main(setup, run, mock_args=("--seed", write_seed()))
