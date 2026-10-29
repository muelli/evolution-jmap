# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: does the preview fetch an http(s) contact photo today?

Item 151 (a). A contact has a photo at an http URL. The scenario selects
the contact in the preview and waits, then checks whether the local http
photo server saw a request. This settles the question for item 151's
design note: do not trust a "no request" result unless this scenario
sees the photo server up and reachable first (checked with a direct
fetch before the Evolution part of the scenario runs), since a routing
problem would also show as "no request" for the wrong reason.

If a request does happen, this is a privacy leak item 151 (b) must fix
before this scenario can pass: a server that controls the photo URL can
use it to detect when, and how often, a person views a contact.
"""

import time
import urllib.request

import contact_photo as cp
from scenario import Jmap, ScenarioFailure, expect_no_errors, main, say, screenshot

NAME = "Helen Httpphoto"
CARDS = [cp.card("photo-fetch", NAME, cp.PHOTO_URL)]
WAIT_SECONDS = 10


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

    server_id = cp.card_ids(server)[NAME]
    if server.get("ContactCard", server_id)["media"]["p1"]["uri"] != cp.PHOTO_URL:
        raise ScenarioFailure("the server lost the photo URL")

    expect_no_errors()
    expect_no_errors("dbus.log")


main(lambda: cp.setup(CARDS), run, component="addressbook")
