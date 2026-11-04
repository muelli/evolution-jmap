# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: the address book preview's Load image placeholder has no
unnamed focusable widget and no duplicate Alt+letter mnemonic.

Item 174(d)'s automated audit (see a11y-audit.py), applied to the
click-to-load placeholder item 151/175 added for an http(s) contact photo
(`external_photo::display_ext`). Scoped to the placeholder's own
container, the button's immediate parent, not the whole preview:
`contact_photo.py`'s own `preview_texts` doc comment records that the
preview's other content reaches AT-SPI as generic text nodes outside
`FOCUSABLE_ROLES`, so a whole-preview walk would audit content well
outside this item's surface for no benefit.
"""

import time

import contact_photo as cp
from scenario import audit_dialog, audit_mnemonics, main, wait_find

NAME = "Helen Httpphoto"
CARDS = [cp.card("photo-fetch", NAME, cp.PHOTO_URL)]
LOAD_BUTTON = "Load image"


def run():
    cp.show_account_book()
    cp.preview(NAME, "preview")
    time.sleep(3)
    button = wait_find(None, LOAD_BUTTON, showing=True, timeout=30)
    placeholder = button.parent
    audit_dialog(placeholder, "Load image placeholder")
    audit_mnemonics(placeholder, "Load image placeholder")


main(lambda: cp.setup(CARDS), run, component="addressbook")
