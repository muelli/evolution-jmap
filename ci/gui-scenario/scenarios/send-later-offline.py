# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: a composer's File > Send Later submenu while offline (ROADMAP
item 176(a), the "Send Later/Outbox offline" row of the offline matrix).

Before this scenario's fix, the Send Later gate (`send_later::extension::
gate`) never checked the shell's online state: it always connected to the
server to ask whether scheduled send is offered, the same way a composer's
own From-identity change re-runs it. In this harness the mock's TCP port
stays open under File > Work Offline (the same caveat `work-offline.py`
documents), so the old code's connection attempt actually succeeded: a
user who believed Send Later was unavailable while offline could still
have it silently reach the server. The fix checks `link::shell_is_offline()`
first and skips the network attempt entirely when offline, the same shape
`settings-pages-offline.py` already covers for the Vacation Responder and
Server Filters pages.

The settle wait below is deliberate: it is well past the mock's normal,
sub-second gate round trip (see `send-later.py`, which waits for the
submenu to become sensitive with no explicit sleep at all), so a reverted
fix makes the submenu end up sensitive within that window and this
scenario fails, not passes late.
"""

import time

from scenario import (
    Jmap,
    ScenarioFailure,
    expect_no_errors,
    find,
    labelled_field,
    main,
    menu_item,
    pyatspi,
    say,
    screenshot,
    select_folder,
    set_text,
    wait_for,
    window,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
RECIPIENT = "bob@example.com"
SUBJECT = "Offline send later"
# Well past the mock's normal gate round trip (sub-second): see the module note.
SETTLE_SECONDS = 5


def setup():
    write_standalone_mail_account(ACCOUNT)
    say("setup: account source written")


def run():
    Jmap()

    select_folder(ACCOUNT, "Inbox")
    say(f"online: {ACCOUNT!r} is synced")

    menu_item("File/Work Offline")
    say("menu: File > Work Offline")

    menu_item("File/New/Mail Message")
    composer = wait_for(lambda: window("Compose Message"), "the composer window")
    set_text(wait_for(lambda: find(composer, "text", "To:"), "the To field"), RECIPIENT)
    set_text(wait_for(lambda: labelled_field(composer, "Subject:"), "the Subject field"), SUBJECT)
    title = wait_for(lambda: window(SUBJECT), f"the composer title shows {SUBJECT!r}")
    screenshot("composer-offline")

    send_later = wait_for(
        lambda: find(title, "menu", "Send Later"), "the Send Later submenu"
    )
    time.sleep(SETTLE_SECONDS)
    if send_later.getState().contains(pyatspi.STATE_SENSITIVE):
        raise ScenarioFailure(
            "Send Later is sensitive while offline: the gate reached the server"
        )
    say("composer: Send Later stays insensitive while offline, no server reached")
    screenshot("send-later-offline-insensitive")

    expect_no_errors()


main(setup, run, mock_args=("--max-delayed-send", "86400"))
