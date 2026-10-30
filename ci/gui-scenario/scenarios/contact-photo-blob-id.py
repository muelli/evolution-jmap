# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: a contact photo the server states as a blobId (RFC 9610 section 2).

The mock turns a `data:` photo into a `blobId` on every ContactCard/set and
ContactCard/get (--contact-photo-data-uri-as-blob-id). See contact_photo.py for
the steps. The checks:

- The server stores the photo as blobId, not uri.
- The client downloads the blob and gives EDS an inline photo. The cache has
  a file: URI of a cache file with the same image bytes, the same shape the
  plain data: URI case shows (contact-photos.py), and the preview paints it.
- After a rename in Evolution, the server still has the same blobId: the
  client does not turn an untouched blobId photo into an edit.
"""

import base64
import urllib.parse

import contact_photo as cp
from scenario import Jmap, ScenarioFailure, expect_no_errors, main, say, screenshot

BLOB = "Blair Blob"
IMAGE = cp.png()
DATA_URI = "data:image/png;base64," + base64.b64encode(IMAGE).decode()
CARDS = [cp.card("photo-blob", BLOB, DATA_URI)]


def file_line(lines):
    if len(lines) != 1:
        return "not one PHOTO line"
    name, _, value = lines[0].partition(":")
    if "VALUE=URI" not in name.upper() or not value.startswith("file://"):
        return "not a file URI"
    with open(urllib.parse.unquote(value.removeprefix("file://")), "rb") as image:
        if image.read() != IMAGE:
            return "not the image of the server"
    return None


def photo_media(stored):
    return stored["media"]["p1"]


def run():
    server = Jmap()
    card_id = cp.card_ids(server)[BLOB]
    before = photo_media(server.get("ContactCard", card_id))
    if "blobId" not in before or "uri" in before:
        raise ScenarioFailure(f"server did not store the photo as blobId: {before}")
    say(f"server: the photo is blobId {before['blobId']!r}")
    cp.show_account_book()

    cp.check_cache(BLOB, file_line)
    cp.preview(BLOB, "preview-blob")

    new_name = f"{BLOB} Renamed"
    cp.edit_name(BLOB, new_name)
    after = photo_media(cp.check_round_trip(server, card_id, new_name))
    if after != before:
        raise ScenarioFailure(f"the rename changed the media: {after}, expected {before}")
    say("server: the blobId photo did not change")
    screenshot("renamed")
    expect_no_errors()
    expect_no_errors("dbus.log")


main(
    lambda: cp.setup(CARDS),
    run,
    component="addressbook",
    mock_args=("--contact-photo-data-uri-as-blob-id",),
)
