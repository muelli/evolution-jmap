# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: what the preview does with an http(s) contact photo.

A contact has a photo at an http URL. The scenario selects the contact in
the preview and waits, then checks three things.

First, whether the local http photo server saw a request. Do not trust a
"no request" result unless this scenario sees the photo server up and
reachable first (checked with a direct fetch before the Evolution part of
the scenario runs), since a routing problem would also show as "no
request" for the wrong reason. If a request does happen, that is a privacy
leak: a server that controls the photo URL can use it to detect when, and
how often, a person views a contact.

Second, whether the preview says so. Stock Evolution shows a broken image
icon and never names the URL. The placeholder of jmap-ui replaces it with
a label and the URL in full, so the user can read where the image claims
to be.

Third, that clicking the "Load image" button is the one thing that makes a
request happen, and that it makes exactly one, not zero and not a retry
storm.
"""

import time
import urllib.request

import contact_photo as cp
from scenario import (
    Jmap,
    ScenarioFailure,
    do_action,
    expect_no_errors,
    main,
    say,
    screenshot,
    wait_find,
    wait_for,
)

NAME = "Helen Httpphoto"
CARDS = [cp.card("photo-fetch", NAME, cp.PHOTO_URL)]
WAIT_SECONDS = 10
# The label of the placeholder, as jmap-ui writes it in English.
PLACEHOLDER = "Externally hosted image"
# The text of the button that fetches the photo, as jmap-ui writes it.
LOAD_BUTTON = "Load image"


def run():
    server = Jmap()
    photos = cp.PhotoServer()

    with urllib.request.urlopen(cp.PHOTO_URL, timeout=5) as response:
        if response.status != 200:
            raise ScenarioFailure(f"the photo server answered {response.status}")
    if photos.requests != ["/photo.png"]:
        raise ScenarioFailure(f"the photo server is not reachable: {photos.requests}")
    photos.requests.clear()
    say("photo server: reachable directly, request log cleared")

    cp.show_account_book()
    cp.preview(NAME, "preview")
    time.sleep(WAIT_SECONDS)
    screenshot("after-wait")

    if photos.requests:
        raise ScenarioFailure(f"the preview fetched the http photo: {photos.requests}")
    say(f"photo server: no request for the http photo after {WAIT_SECONDS}s")

    texts = cp.wait_for_text(PLACEHOLDER, "the placeholder of the external photo")
    if not any(cp.PHOTO_URL in text for text in texts):
        raise ScenarioFailure(f"the placeholder does not name the URL: {texts}")
    say(f"preview: the placeholder names {cp.PHOTO_URL}")
    if photos.requests:
        raise ScenarioFailure(f"the placeholder fetched the http photo: {photos.requests}")

    server_id = cp.card_ids(server)[NAME]
    if server.get("ContactCard", server_id)["media"]["p1"]["uri"] != cp.PHOTO_URL:
        raise ScenarioFailure("the server lost the photo URL")

    button = wait_find(None, LOAD_BUTTON, showing=True)
    do_action(button)
    say(f"clicked {LOAD_BUTTON!r}")

    wait_for(lambda: photos.requests or None, "the photo server to see the click's request")
    time.sleep(2)
    screenshot("after-click")
    if photos.requests != ["/photo.png"]:
        raise ScenarioFailure(f"the click made an unexpected request shape: {photos.requests}")
    say(f"photo server: exactly one request after the click: {photos.requests}")

    expect_no_errors()
    expect_no_errors("dbus.log")


main(lambda: cp.setup(CARDS), run, component="addressbook")
