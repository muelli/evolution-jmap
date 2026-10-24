# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""The delete of a shared calendar or address book, with its undo window.

The scenarios shared-calendar-delete and shared-book-delete call run_scenario.
The steps and checks:

1. Delete the shared collection. An info bar with Undo shows, and the server
   still has the collection.
2. Undo. The collection shows again, and the server still has it after the
   end of the first window.
3. Delete again. A new info bar shows, with its own window: the server still
   has the collection after the end of the first window.
4. Wait. At the end of the second window, the server destroys the collection.
   The collection of the same id in the user's own account stays.
"""

import time

from scenario import (
    Jmap,
    ScenarioFailure,
    click_button,
    confirm,
    expect_log,
    expect_no_errors,
    info_bar,
    info_bar_texts,
    menu_item,
    say,
    screenshot,
    select_source,
    source_cell,
    wait_for,
    write_collection_account,
)

# The mock has no plain shared account. This mode adds the shared account
# "A2", with the name of the user.
MOCK_ARGS = ("--shared-account-name-copies-user",)
SHARED_ACCOUNT = "A2"
SHARED_NAME = "alice@example.com"

# The undo window of the registry, in seconds.
WINDOW = 60
# The time after the end of a window in which the destroy must arrive.
DESTROY_TIMEOUT = 30
DESTROYED = "deleted collection resource on server"


class Kind:
    """What differs between a calendar and an address book."""

    def __init__(self, label, type_name, menu, collection, rights):
        self.label = label
        self.type_name = type_name
        self.menu = menu
        self.collection = collection
        # The registry offers a shared collection for delete only with
        # myRights.mayDelete. The mock returns the rights that the create sets.
        self.rights = rights


CALENDAR = Kind(
    "Calendar",
    "Calendar",
    "Edit/Delete Calendar",
    "Team calendar",
    {
        "mayReadFreeBusy": True,
        "mayReadItems": True,
        "mayWriteAll": True,
        "mayWriteOwn": True,
        "mayUpdatePrivate": True,
        "mayRSVP": True,
        "mayShare": True,
        "mayDelete": True,
    },
)
ADDRESS_BOOK = Kind(
    "Address book",
    "AddressBook",
    "Edit/Delete Address Book",
    "Team contacts",
    {"mayRead": True, "mayWrite": True, "mayShare": True, "mayDelete": True},
)


def setup(kind):
    write_collection_account()
    collection_id = Jmap(account_id=SHARED_ACCOUNT).create(
        kind.type_name, {"name": kind.collection, "myRights": kind.rights}
    )
    say(f"setup: collection account written, shared {kind.type_name} {collection_id} created")


def run(kind):
    shared = Jmap(account_id=SHARED_ACCOUNT)
    own = Jmap()
    collection_id = next(i for i, n in shared.names(kind.type_name).items() if n == kind.collection)
    own_ids = set(own.names(kind.type_name))
    display = f"{SHARED_NAME}: {kind.collection}"
    headline = f'{kind.label} "{display}" is deleted.'

    def on_server():
        return collection_id in shared.names(kind.type_name)

    def delete(step):
        select_source(display)
        screenshot(f"{step}-before-delete")
        menu_item(kind.menu)
        confirm("Delete From Server", f"{step}-delete-confirmation")
        started = time.monotonic()
        bar = wait_for(lambda: info_bar(headline), f"the info bar {headline!r}")
        texts = info_bar_texts(bar)
        say(f"info bar: {texts}")
        if not any("for all users who share it" in text for text in texts):
            raise ScenarioFailure(f"the info bar does not say that the delete is for all users: {texts}")
        wait_for(lambda: source_cell(display) is None, f"{display!r} hidden in the source selector")
        screenshot(f"{step}-info-bar")
        if not on_server():
            raise ScenarioFailure("the server destroyed the collection inside the undo window")
        say("server: the collection is still there")
        return bar, started

    def sleep_until(moment):
        left = moment - time.monotonic()
        if left > 0:
            time.sleep(left)

    bar, first = delete("first")
    click_button(bar, "Undo")
    wait_for(lambda: source_cell(display), f"{display!r} shown again in the source selector")
    wait_for(lambda: info_bar(headline) is None, "the info bar closed")
    screenshot("after-undo")

    # A gap between the two deletes tells the two windows apart.
    sleep_until(first + 10)
    bar, second = delete("second")
    sleep_until(first + WINDOW + 3)
    if not on_server():
        raise ScenarioFailure("the server destroyed the collection at the end of the first window")
    say("server: the collection outlived the first window")
    screenshot("second-info-bar-after-first-window")

    sleep_until(second + WINDOW - 3)
    if not on_server():
        raise ScenarioFailure("the server destroyed the collection before the end of the second window")
    say("server: the collection is still there just before the end of the second window")
    wait_for(lambda: not on_server(), "the destroy on the server", timeout=DESTROY_TIMEOUT)
    destroyed_after = time.monotonic() - second
    say(f"server: the collection is destroyed {destroyed_after:.0f}s after the second delete")
    if set(own.names(kind.type_name)) != own_ids:
        raise ScenarioFailure("the destroy changed the user's own collections")
    say("server: the user's own collections are unchanged")
    expect_log(DESTROYED, "the destroy", log="dbus.log")
    wait_for(lambda: info_bar(headline) is None, "the info bar closed")
    screenshot("after-destroy")
    expect_no_errors()
    expect_no_errors("dbus.log")
