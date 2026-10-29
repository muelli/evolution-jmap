# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Contact photos from the server in the address book of Evolution.

The scenarios contact-photos and contact-photo-file call the functions here.
For each contact of the setup, the steps and checks are:

1. The contact shows in the address book. The local cache of Evolution has
   the expected PHOTO line, or no PHOTO line.
2. Select the contact. The preview shows the contact (screenshot).
3. Change the full name in the contact editor and save. The server has the
   new name, and the photo on the server does not change.
"""

import glob
import http.server
import os
import sqlite3
import struct
import threading
import time
import zlib

from scenario import (
    Jmap,
    ScenarioFailure,
    click_button,
    descendants,
    double_click,
    evolution,
    is_showing,
    pyatspi,
    say,
    screenshot,
    set_text,
    text_field,
    text_of,
    wait_find,
    wait_for,
    window,
    write_collection_account,
)

# The scenario serves the http photo on this port of the local host.
PHOTO_PORT = 8091
PHOTO_URL = f"http://127.0.0.1:{PHOTO_PORT}/photo.png"
EDITOR = "Contact Editor"
PREVIEW_SECONDS = 3


def png(width=64, height=64, rgb=(200, 60, 40)):
    """Return a PNG image of one colour."""

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    row = b"\x00" + bytes(rgb) * width
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(row * height))
        + chunk(b"IEND", b"")
    )


def card(uid, full_name, photo_uri):
    """Return a ContactCard with a photo at `photo_uri`."""
    return {
        "@type": "Card",
        "version": "1.0",
        "uid": uid,
        "addressBookIds": {"AB1": True},
        "name": {"full": full_name},
        "media": {
            "p1": {"@type": "Media", "kind": "photo", "uri": photo_uri, "mediaType": "image/png"}
        },
    }


def setup(cards):
    """Write the account and create `cards` on the server."""
    write_collection_account()
    server = Jmap()
    for item in cards:
        card_id = server.create("ContactCard", item)
        say(f"setup: ContactCard {card_id} {item['name']['full']!r} created")


class PhotoServer:
    """An http server for PHOTO_URL that counts the requests."""

    def __init__(self):
        image = png()
        requests = self.requests = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):  # noqa: N802
                requests.append(self.path)
                self.send_response(200)
                self.send_header("Content-Type", "image/png")
                self.send_header("Content-Length", str(len(image)))
                self.end_headers()
                self.wfile.write(image)

            def log_message(self, *_):
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", PHOTO_PORT), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()


def cached_vcard(full_name):
    """Return the vCard of `full_name` in the address book cache of Evolution, or None.

    EDS keeps an inline photo of the cache in a file and puts a file: URI
    to that file in the vCard.
    """
    cache = os.environ["XDG_CACHE_HOME"]
    for path in glob.glob(os.path.join(cache, "evolution", "addressbook", "*", "cache.db")):
        connection = sqlite3.connect(f"file:{path}?mode=ro", uri=True, timeout=5)
        try:
            rows = connection.execute("SELECT ECacheOBJ FROM ECacheObjects").fetchall()
        finally:
            connection.close()
        for (vcard,) in rows:
            if f"\nFN:{full_name}\r" in vcard or f"\nFN:{full_name}\n" in vcard:
                return vcard
    return None


def photo_lines(vcard):
    """Return the PHOTO lines of `vcard`, unfolded."""
    unfolded = vcard.replace("\r\n ", "").replace("\n ", "")
    return [line for line in unfolded.splitlines() if line.upper().startswith("PHOTO")]


def show_account_book():
    """Select the address book of the account, not the local "Personal" above it."""

    def jmap_book():
        cells = [
            cell
            for cell in descendants(evolution(), "table cell")
            if cell.name == "Personal" and is_showing(cell)
        ]
        return cells[-1] if len(cells) > 1 else None

    wait_for(jmap_book, "the address book of the account").queryComponent().grabFocus()


def click(node):
    """Click the centre of `node` with the synthetic mouse."""
    extents = node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
    pyatspi.Registry.generateMouseEvent(
        extents.x + extents.width // 2, extents.y + extents.height // 2, "b1c"
    )


def check_cache(full_name, expect):
    """Wait for the cached vCard of `full_name` and check its PHOTO lines with `expect`.

    `expect` takes the list of PHOTO lines and returns an error text, or None.
    """
    vcard = wait_for(lambda: cached_vcard(full_name), f"{full_name!r} in the cache", timeout=30)
    lines = photo_lines(vcard)
    error = expect(lines)
    if error is not None:
        raise ScenarioFailure(f"cache of {full_name!r}: {error}: {[line[:80] for line in lines]}")
    say(f"cache: {full_name!r} has {[line[:60] for line in lines]}")


def preview(full_name, label):
    """Select the contact, so that the preview shows it, and save a screenshot."""
    item = wait_find("label", full_name, showing=True, timeout=90)
    click(item)
    # The preview is a web view. Give it time to load the image.
    time.sleep(PREVIEW_SECONDS)
    screenshot(label)


def preview_texts():
    """Return the accessible text of every node of Evolution that has text.

    The preview is a web view, so its content reaches AT-SPI as text nodes
    with no accessible name. A search by name cannot find them.
    """
    found = []
    for node in descendants(evolution()):
        try:
            found.append(text_of(node))
        except Exception:  # noqa: BLE001
            continue
    return found


def wait_for_text(needle, what, timeout=30):
    """Wait until a node of Evolution shows `needle` and return every text."""

    def showing():
        texts = preview_texts()
        return texts if any(needle in text for text in texts) else None

    return wait_for(showing, what, timeout)


def edit_name(full_name, new_name):
    """Change the full name of the contact in the contact editor and save."""
    item = wait_find("label", full_name, showing=True)
    double_click(item)
    editor = wait_for(lambda: window(EDITOR), "the contact editor")
    field = wait_for(lambda: text_field(editor, full_name), f"the field with {full_name!r}")
    set_text(field, new_name)
    screenshot(f"edit-{new_name.split()[0].lower()}")
    click_button(editor, "Save")
    wait_for(lambda: window(EDITOR) is None, "the editor closed")


def check_round_trip(server, card_id, new_name):
    """Wait for `new_name` on the server and return the card."""

    def renamed():
        stored = server.get("ContactCard", card_id)
        return stored if (stored.get("name") or {}).get("full") == new_name else None

    stored = wait_for(renamed, f"the name {new_name!r} on the server", timeout=30)
    say(f"server: has the name {new_name!r}")
    return stored


def card_ids(server):
    """Return {full name: id} of the cards on the server."""
    found = server.call("ContactCard/get", {"ids": None})["list"]
    return {(item.get("name") or {}).get("full"): item["id"] for item in found}
