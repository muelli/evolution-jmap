# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: a contact whose photo the server gives as a local file: URI.

The mock gives each photo as a file: URI (--contact-photo-uses-file-uri).
See contact_photo.py for the steps. The checks:

- The cache has no PHOTO line, so Evolution never reads the local file.
- After a rename in Evolution, the server still has its photo entry: the
  client does not remove or duplicate a photo that it hides.
"""

import contact_photo as cp
from scenario import Jmap, ScenarioFailure, expect_no_errors, main, say, screenshot

FILE = "Fiona File"
CARDS = [cp.card("photo-file", FILE, cp.PHOTO_URL)]


def no_line(lines):
    return "a PHOTO line" if lines else None


def run():
    server = Jmap()
    card_id = cp.card_ids(server)[FILE]
    before = server.get("ContactCard", card_id)["media"]
    say(f"server: the photo is {before['p1']['uri']!r}")
    cp.show_account_book()

    cp.check_cache(FILE, no_line)
    cp.preview(FILE, "preview-file")

    new_name = f"{FILE} Renamed"
    cp.edit_name(FILE, new_name)
    after = cp.check_round_trip(server, card_id, new_name)["media"]
    if after != before:
        raise ScenarioFailure(f"the rename changed the media: {after}, expected {before}")
    say("server: the hidden photo did not change")
    screenshot("renamed")
    expect_no_errors()
    expect_no_errors("dbus.log")


main(
    lambda: cp.setup(CARDS),
    run,
    component="addressbook",
    mock_args=("--contact-photo-uses-file-uri",),
)
