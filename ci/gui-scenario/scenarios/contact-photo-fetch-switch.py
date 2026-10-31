# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: a slow "Load image" fetch must not show in another contact.

Two contacts have photos at different http URLs. The scenario clicks
"Load image" for the first contact, and the photo server holds that
response. While it waits, the scenario selects the second contact, so the
preview loads a new document in the same web view. Then the photo server
answers.

The photo of the first contact must not replace the placeholder of the
second contact. The preview must still name the URL of the second contact
and offer its own "Load image" button.
"""

import time

import contact_photo as cp
from scenario import (
    ScenarioFailure,
    do_action,
    expect_no_errors,
    main,
    say,
    screenshot,
    wait_find,
    wait_for,
)

FIRST = "Fiona Firstphoto"
SECOND = "Sven Secondphoto"
FIRST_PATH = "/first.png"
SECOND_PATH = "/second.png"
FIRST_URL = f"http://127.0.0.1:{cp.PHOTO_PORT}{FIRST_PATH}"
SECOND_URL = f"http://127.0.0.1:{cp.PHOTO_PORT}{SECOND_PATH}"
CARDS = [
    cp.card("photo-switch-first", FIRST, FIRST_URL),
    cp.card("photo-switch-second", SECOND, SECOND_URL),
]
PLACEHOLDER = "Externally hosted image"
LOAD_BUTTON = "Load image"
# Time for the finish of the fetch to reach the web view.
SETTLE_SECONDS = 5


def run():
    photos = cp.PhotoServer(held={FIRST_PATH})

    cp.show_account_book()
    cp.preview(FIRST, "first")
    texts = cp.wait_for_text(FIRST_URL, "the placeholder of the first contact")
    if not any(PLACEHOLDER in text for text in texts):
        raise ScenarioFailure(f"the first contact has no placeholder: {texts}")

    do_action(wait_find(None, LOAD_BUTTON, showing=True))
    say(f"clicked {LOAD_BUTTON!r} for {FIRST!r}")
    wait_for(lambda: FIRST_PATH in photos.requests or None, "the request of the click")
    say(f"photo server: holds the answer for {FIRST_PATH}")

    cp.preview(SECOND, "second")
    cp.wait_for_text(SECOND_URL, "the placeholder of the second contact")
    say(f"preview: shows the placeholder of {SECOND!r}")

    photos.release.set()
    wait_for(lambda: FIRST_PATH in photos.answered or None, "the held answer")
    time.sleep(SETTLE_SECONDS)
    screenshot("after-release")

    texts = cp.preview_texts()
    if not any(SECOND_URL in text for text in texts):
        raise ScenarioFailure(
            f"the photo of {FIRST!r} replaced the placeholder of {SECOND!r}: {texts}"
        )
    wait_find(None, LOAD_BUTTON, showing=True, timeout=5)
    say(f"preview: still shows the placeholder of {SECOND!r} and its button")
    if SECOND_PATH in photos.requests:
        raise ScenarioFailure(f"the preview fetched {SECOND_PATH} with no click")

    expect_no_errors()
    expect_no_errors("dbus.log")


main(lambda: cp.setup(CARDS), run, component="addressbook")
