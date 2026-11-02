# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: the context menu's Snooze submenu has no unnamed focusable
menu item.

Item 174(d)'s automated audit (see a11y-audit.py), applied to the one
surface a context menu holds. `snooze.py` already opens this submenu and
waits for it to go sensitive, but its module-level `main(...)` call runs
on import (no `if __name__ == "__main__"` guard), so this scenario does
not import it; it repeats the small setup and popup steps itself, the
same choice `snooze-preview.py` already made for its own local `ESCAPE`
constant instead of sharing snooze.py's.

A collapsed submenu's menu items stay in the AT-SPI tree but without
`STATE_SHOWING` set (`menu_item()`'s own docstring: "the menus need not
open" to find and activate one), the same way a hidden `EDateEdit` time
combo does; `unnamed_widgets` would silently skip them. So this scenario
clicks the Snooze item open before auditing its children, not only
waiting for it to go sensitive.
"""

import pyatspi

from scenario import (
    audit_dialog,
    click_at,
    evolution,
    find,
    main,
    popup_menu,
    say,
    select_folder,
    wait_for,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
SUBJECT = "Welcome to jmap-mockd"
ESCAPE = 0xFF1B


def setup():
    write_standalone_mail_account(ACCOUNT)
    say("setup: account sources written")


def run():
    select_folder(ACCOUNT, "Inbox")
    popup_menu(SUBJECT)
    submenu = wait_for(lambda: find(evolution(), "menu", "Snooze"), "the Snooze submenu")
    wait_for(
        lambda: submenu.getState().contains(pyatspi.STATE_SENSITIVE),
        "the Snooze submenu sensitive (the gate found the extension)",
    )
    click_at(submenu)
    wait_for(
        lambda: any(
            find(submenu, "menu item", name, showing=True) is not None
            for name in ("For One Hour", "Until Tomorrow Morning", "Until Next Monday Morning")
        ),
        "a showing preset of the open Snooze submenu",
    )
    audit_dialog(submenu, "Snooze")
    pyatspi.Registry.generateKeyboardEvent(ESCAPE, None, pyatspi.KEY_SYM)
    pyatspi.Registry.generateKeyboardEvent(ESCAPE, None, pyatspi.KEY_SYM)
    say("closed the submenu and the context menu")


main(setup, run, mock_args=("--snooze-extension",))
