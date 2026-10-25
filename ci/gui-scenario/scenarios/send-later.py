# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: File > Send Later in the composer of a JMAP account.

1. Alice schedules a message "In One Hour". The server holds it: one
   EmailSubmission with Alice's identity, the "pending" undo status and a
   sendAt one hour ahead. The message is in Sent, not a draft. The composer
   closes, and the Sent folder shows the message.
2. A second account sends as an address that the server has no identity
   for. Send Later refuses with a dialog, as Send does. The server gets no
   submission and no message. The composer stays open.

The mock offers FUTURERELEASE with a maxDelayedSend of one day. Both accounts
use the same mock account, which has only the identity alice@example.com.
"""

import time

from scenario import (
    Jmap,
    ScenarioFailure,
    click_button,
    descendants,
    evolution,
    expect_log,
    expect_no_errors,
    find,
    is_showing,
    labelled_field,
    main,
    menu_item,
    message_list,
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
OTHER = "JMAP scenario other"
ALICE = "alice@example.com"
MALLORY = "mallory@example.com"
RECIPIENT = "bob@example.com"
SCHEDULED = "Scheduled hello"
REFUSED = "Not my address"
ONE_HOUR = 3600
# The mock has a fixed clock: its sendAt is this time plus the HOLDFOR value.
MOCK_NOW = "2026-01-01T00:00:00Z"
# The client computes the hold from the local clock, so allow some seconds.
SLACK = 60


def setup():
    write_standalone_mail_account(ACCOUNT)
    write_standalone_mail_account(OTHER, key="other", identity=("Mallory", MALLORY))
    say("setup: two account sources written, one with an address the server does not know")


def compose(account, subject):
    """Open a composer in `account`, fill To and Subject, and return its window title.

    The window title becomes the subject. Wait until the Send Later gate has
    made the submenu sensitive.
    """
    select_folder(account, "Inbox")
    menu_item("File/New/Mail Message")
    composer = wait_for(lambda: window("Compose Message"), "the composer window")
    set_text(wait_for(lambda: find(composer, "text", "To:"), "the To field"), RECIPIENT)
    set_text(wait_for(lambda: labelled_field(composer, "Subject:"), "the Subject field"), subject)
    wait_for(lambda: window(subject), f"the composer title shows {subject!r}")
    send_later = wait_for(
        lambda: find(window(subject), "menu", "Send Later"), "the Send Later submenu"
    )
    wait_for(
        lambda: send_later.getState().contains(pyatspi.STATE_SENSITIVE),
        "the Send Later submenu sensitive (the gate found FUTURERELEASE)",
    )
    say(f"composer {subject!r} ready, Send Later is sensitive")
    return subject


def submissions_of(server, subject):
    """Return the submissions of the message with `subject`."""
    email = server.emails().get(subject)
    if email is None:
        return []
    return [s for s in server.submissions() if s["emailId"] == email["id"]]


def utc_seconds(stamp):
    """Seconds since the epoch of an RFC 3339 UTC date like 2026-10-24T14:00:00Z."""
    return time.mktime(time.strptime(stamp, "%Y-%m-%dT%H:%M:%SZ")) - time.timezone


def alert_with(prefix):
    """Return the showing alert of Evolution with a label that starts with `prefix`, or None."""
    for node in descendants(evolution(), "alert", max_depth=2):
        if is_showing(node) and find(node, "label", prefix=prefix):
            return node
    return None


def run():
    server = Jmap()
    alice_identity = server.identities()[ALICE]["id"]
    drafts = server.mailbox_id("drafts")
    sent = server.mailbox_id("sent")
    if MALLORY in server.identities():
        raise ScenarioFailure(f"the mock has an identity for {MALLORY}")

    # 1. Alice schedules a message.
    title = compose(ACCOUNT, SCHEDULED)
    screenshot("composer-before-send-later")
    menu_item("File/Send Later/In One Hour", title)
    submission = wait_for(
        lambda: (submissions_of(server, SCHEDULED) or [None])[0],
        f"a submission of {SCHEDULED!r} on the server",
    )
    say(f"server: submission {submission['id']}, sendAt {submission.get('sendAt')}")
    if submission["identityId"] != alice_identity:
        raise ScenarioFailure(f"the submission uses identity {submission['identityId']}")
    if submission.get("undoStatus") != "pending":
        raise ScenarioFailure(f"the server does not hold the message: {submission}")
    hold = utc_seconds(submission["sendAt"]) - utc_seconds(MOCK_NOW)
    if not ONE_HOUR - SLACK <= hold <= ONE_HOUR + SLACK:
        raise ScenarioFailure(f"the hold is {hold:.0f}s, not one hour")
    envelope = submission.get("envelope") or {}
    if (envelope.get("mailFrom") or {}).get("email") != ALICE:
        raise ScenarioFailure(f"the envelope sender is not {ALICE}: {envelope}")
    if [r["email"] for r in envelope.get("rcptTo") or []] != [RECIPIENT]:
        raise ScenarioFailure(f"the envelope recipients are not [{RECIPIENT}]: {envelope}")
    say(f"server: held for {hold:.0f}s, identity {ALICE}, envelope to {RECIPIENT}")

    email = server.get("Email", submission["emailId"])
    if set(email["mailboxIds"]) != {sent}:
        raise ScenarioFailure(f"the message is not only in Sent: {email['mailboxIds']}")
    if drafts in email["mailboxIds"] or email.get("keywords", {}).get("$draft"):
        raise ScenarioFailure(f"the message is still a draft: {email}")
    say("server: the message is in Sent and is not a draft")
    wait_for(lambda: window(title) is None, "the composer closes")
    expect_log(r"jmap_ui.*message scheduled; closing the composer", "the scheduled send")
    select_folder(ACCOUNT, "Sent")
    wait_for(lambda: find(message_list(), "table cell", SCHEDULED), f"{SCHEDULED!r} in Sent")
    say(f"Evolution shows {SCHEDULED!r} in Sent")
    screenshot("sent-after-send-later")

    # 2. A sender that the server has no identity for.
    title = compose(OTHER, REFUSED)
    sender = find(window(title), "combo box", prefix="Mallory")
    if sender is None:
        raise ScenarioFailure(f"the composer of {OTHER!r} does not send as Mallory")
    before = len(server.submissions())
    menu_item("File/Send Later/In One Hour", title)
    alert = wait_for(
        lambda: alert_with("The message could not be scheduled"), "the refusal dialog"
    )
    text = " ".join(label.name or "" for label in descendants(alert, "label"))
    if f"cannot send mail as {MALLORY}" not in text:
        raise ScenarioFailure(f"the refusal does not name the sender: {text!r}")
    say(f"refusal: {text.strip()!r}")
    screenshot("send-later-refused")
    click_button(alert, "Close")
    wait_for(lambda: alert_with("The message could not be scheduled") is None, "the dialog closes")
    if len(server.submissions()) != before:
        raise ScenarioFailure("the server got a submission for the refused sender")
    if REFUSED in server.emails():
        raise ScenarioFailure(f"the server got the message {REFUSED!r}")
    if window(title) is None:
        raise ScenarioFailure("the composer closed after the refusal")
    say("server: no submission and no message; the composer stays open")
    screenshot("composer-after-refusal")
    expect_no_errors()


main(setup, run, mock_args=("--max-delayed-send", "86400"))
