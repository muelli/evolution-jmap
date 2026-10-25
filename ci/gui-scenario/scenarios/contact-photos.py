# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: contacts with an http photo and with a data photo.

See contact_photo.py for the steps. The checks:

- The cache has "PHOTO;VALUE=uri" with the http URL for the http photo. For
  the data photo, it has a file: URI of a cache file with the same image bytes.
- The preview does not fetch the http photo. Evolution 3.52 shows a broken
  image for it, so a server URL does not reveal when the user looks.
- After a rename in Evolution, the server has the same photo URI as before.
"""

import base64
import urllib.parse

import contact_photo as cp
from scenario import Jmap, ScenarioFailure, expect_no_errors, main, say, screenshot

HTTP = "Hannah Http"
DATA = "Dana Data"
IMAGE = cp.png()
DATA_URI = "data:image/png;base64," + base64.b64encode(IMAGE).decode()
CARDS = [cp.card("photo-http", HTTP, cp.PHOTO_URL), cp.card("photo-data", DATA, DATA_URI)]


def http_line(lines):
    if len(lines) != 1:
        return "not one PHOTO line"
    name, _, value = lines[0].partition(":")
    if "VALUE=URI" not in name.upper() or value != cp.PHOTO_URL:
        return "not the http URI"
    return None


def data_line(lines):
    if len(lines) != 1:
        return "not one PHOTO line"
    name, _, value = lines[0].partition(":")
    if "VALUE=URI" not in name.upper() or not value.startswith("file://"):
        return "not a file URI"
    with open(urllib.parse.unquote(value.removeprefix("file://")), "rb") as image:
        if image.read() != IMAGE:
            return "not the image of the server"
    return None


def photo_uri(stored):
    return stored["media"]["p1"]["uri"]


def run():
    server = Jmap()
    photos = cp.PhotoServer()
    ids = cp.card_ids(server)
    cp.show_account_book()

    cp.check_cache(HTTP, http_line)
    cp.check_cache(DATA, data_line)

    cp.preview(DATA, "preview-data")
    cp.preview(HTTP, "preview-http")
    if photos.requests:
        raise ScenarioFailure(f"the preview fetched the http photo: {photos.requests}")
    say("photo server: no request for the http photo")

    for name in (HTTP, DATA):
        before = photo_uri(server.get("ContactCard", ids[name]))
        new_name = f"{name} Renamed"
        cp.edit_name(name, new_name)
        after = photo_uri(cp.check_round_trip(server, ids[name], new_name))
        if after != before:
            raise ScenarioFailure(f"the rename of {name!r} changed the photo: {after[:60]!r}")
        say(f"server: the photo of {name!r} did not change")
    screenshot("renamed")
    expect_no_errors()
    expect_no_errors("dbus.log")


main(lambda: cp.setup(CARDS), run, component="addressbook")
