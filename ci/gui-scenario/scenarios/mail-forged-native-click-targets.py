# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: a mail body forges Evolution's own native mail click targets.

Item 178's design note points out why jmap-ui's own click mechanism keys a
button's `value` to an unguessable per-render token: Evolution's native mail
click handlers (`e_web_view_register_element_clicked`) match a click by
class name across every frame of the view, with no check that the element
came from the part it claims to belong to. A sender's HTML body can put an
element of any registered class anywhere on the page. This scenario builds
such a body and records, for each native target it covers, whether a forged
element reaches real behaviour.

`attachment-expander` and `attachment-menu` (`mail/e-mail-display.c`,
registered on every message) look the clicked element's `value` up against
the live in-memory addresses of the message's own `EAttachment` objects
(`mail_display_ref_attachment_from_element`), formatted as a bare `%p`. A
forged value a sender writes into a static message can only guess at an
address that exists in a future reader's process; this scenario checks that
a plausible-looking but wrong one changes nothing and opens no menu.

`__evo-remote-content-img`'s handler (`mail_display_remote_content_clicked_cb`)
reads none of the clicked element's id, class or value: it only emits a
signal that shows the same remote-content popover a real indicator would. A
forged element of this class can make the popover appear unprompted, but
carries no attacker data into it.

`secure-button`, `secure-button-details`, `secure-button-view-certificate`
and `secure-button-import-certificate` (`em-format/e-mail-part-secure-
button.c`) only register at all on a message with a real signed or
encrypted part, so a prior session left them unreproduced. This session
adds `ci/gui-scenario/fixtures/mail-forged-secure-button-signed.eml`, a
real OpenPGP `multipart/signed` message (a throwaway test key, detached-
signed and canonicalised exactly as Camel's own verify path expects; the
mock container carries no public key for it, so `gpg_verify_sync` reports
"no public key" but still returns a validity, not a `GError`, since it
found a real signature packet: read `camel-gpg-context.c`'s own
`gpg_verify_sync` to confirm this does not need the signer's key present
to register the button at all). `secure-button` checks `value` against
`g_strdup_printf("%p:", mail_part)` plus a live `CamelCipherValidity`
pointer; `secure-button-details` checks the clicked element's `id` the
same way. Both read the forged value only to look it up, never to act on
it, so a plausible-but-wrong value (the same shape as attachment-expander
above) matches nothing and the handler returns immediately.
`secure-button-view-certificate`/`-import-certificate` only register
under `ENABLE_SMIME` and look up `CamelCipherCertInfo::cert_data`, which a
PGP signature's validity never carries, so even a real validity pointer
finds no certificate to act on; forged anyway, same wrong-value shape.

`itip-button` (`modules/itip-formatter/itip-view.c`) registers once the
message carries any `text/calendar` part: `e-mail-part-itip.c`'s own
`content_loaded` builds an `ItipView` and calls `itip_view_set_web_view`
unconditionally, which always ends in `itip_view_register_clicked_
listener`, with no dependency on the invitation parsing cleanly or a
matching calendar being configured. This session adds
`ci/gui-scenario/fixtures/mail-forged-itip-button.eml`, a real
`METHOD:REQUEST` `VEVENT` addressed to the reader, so the listener
registers from a genuine invitation. `itip_view_itip_button_clicked_cb`
checks `value` against `g_strdup_printf("%p:", view->priv->itip_part_ptr)`,
the same shape as `secure-button`; a plausible-but-wrong value fails the
prefix check and the handler returns before running any script.

Not covered here, left for a follow-up claim:

`org-gnome-vcard-save-button` (`modules/vcard-inline/e-mail-formatter-
vcard.c`) checks the clicked value with `strstr(part_id, element_value)`,
true for an empty `value`, so the real button's own `value` is the plain
`part_id` string, not a secret pointer; a forged copy with no `value`
attribute at all would pass that check anywhere on the page. This was not
reproduced here: a debug probe clicking the message's own real "Save To
Addressbook" button through `click_at` (the synthetic-mouse path the
in-flight `gtk_dialog_run` requires, since the click reaches it through a
WebKit accessible node) also failed to open the dialog, so the gap is in
this harness's click delivery for that specific control, not evidence
either way about the forged case. Needs a different interaction path
(for example driving the click through `webkit_web_view_run_javascript`,
or a real X11 click through the compositor) before it can be measured.
"""

import base64
import json
import os
import time

from scenario import (
    ScenarioFailure,
    do_action,
    evolution,
    find,
    main,
    menu_item,
    say,
    screenshot,
    select_folder,
    select_message,
    wait_find,
    wait_for,
    window,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
SUBJECT = "Forged native click targets"
WAIT_SECONDS = 10

EXPANDER_TEXT = "Forged Attachment Expander"
MENU_TEXT = "Forged Attachment Menu"
REMOTE_TEXT = "Forged Remote Content"

# A plausible-looking but wrong value: the shape of a real pointer
# (`g_strdup_printf("%p", ...)`), never one this process actually allocated.
FAKE_POINTER = "0x55fdead0beef"

# attachment-expander, attachment-menu and __evo-remote-content-img are all
# registered for the top-level document on every message
# (`mail/e-mail-display.c`'s own `content_loaded`), so a plain HTML body is
# enough to reach every one of them.
HTML_BODY = (
    "<p>Forged native click targets test (item 178).</p>"
    f'<button type="button" class="attachment-expander" '
    f'value="{FAKE_POINTER}">{EXPANDER_TEXT}</button>'
    f'<button type="button" class="attachment-menu" '
    f'value="{FAKE_POINTER}">{MENU_TEXT}</button>'
    f'<button type="button" class="__evo-remote-content-img" '
    f'value="">{REMOTE_TEXT}</button>'
)

SEED_PATH = os.path.join(
    os.environ.get("GUI_SCENARIO_ARTIFACTS", "."), "mail-forged-native-seed.json"
)

SIGNED_SUBJECT = "Forged native click targets (signed)"
SIGNED_FIXTURE = os.path.join(
    os.path.dirname(__file__), "..", "fixtures", "mail-forged-secure-button-signed.eml"
)

SECURE_BUTTON_TEXT = "Forged Secure Button"
SECURE_DETAILS_TEXT = "Forged Secure Button Details"
SECURE_VIEW_CERT_TEXT = "Forged View Certificate"
SECURE_IMPORT_CERT_TEXT = "Forged Import Certificate"

ITIP_SUBJECT = "Forged native click targets (invitation)"
ITIP_FIXTURE = os.path.join(
    os.path.dirname(__file__), "..", "fixtures", "mail-forged-itip-button.eml"
)
ITIP_BUTTON_TEXT = "Forged Itip Button"


def write_seed():
    """Write the `--seed` JSON: one HTML message forging three native click
    targets in its body, a real OpenPGP-signed message forging the
    `secure-button` family, and a real calendar invitation forging
    `itip-button` (`rawSourceBase64`: the mock stores this verbatim as the
    `message/rfc822` blob a real Camel client downloads, `jmap-
    mock::EmailSeed::raw_source`, since `rfc5322()` cannot build a
    `multipart/signed` or `multipart/mixed` shape itself).
    """
    with open(SIGNED_FIXTURE, "rb") as fixture:
        signed_raw = fixture.read()
    with open(ITIP_FIXTURE, "rb") as fixture:
        itip_raw = fixture.read()

    seed = {
        "accounts": [
            {
                "id": "A1",
                "name": "alice@example.com",
                "mailboxes": [
                    {"id": "M1", "name": "Inbox", "role": "inbox"},
                    {"id": "M2", "name": "Sent", "role": "sent"},
                    {"id": "M3", "name": "Drafts", "role": "drafts"},
                ],
                "emails": [
                    {
                        "mailboxId": "M1",
                        "from": {"name": "Sender", "email": "sender@example.test"},
                        "to": [{"name": "Alice", "email": "alice@example.com"}],
                        "subject": SUBJECT,
                        "textBody": HTML_BODY,
                        "bodyIsHtml": True,
                        "receivedAt": "2026-01-02T03:04:05Z",
                    },
                    {
                        "mailboxId": "M1",
                        "from": {"name": "Sender", "email": "sender@example.test"},
                        "to": [{"name": "Alice", "email": "alice@example.com"}],
                        "subject": SIGNED_SUBJECT,
                        "textBody": "(raw_source overrides this)",
                        "receivedAt": "2026-01-02T03:04:06Z",
                        "rawSourceBase64": base64.b64encode(signed_raw).decode("ascii"),
                    },
                    {
                        "mailboxId": "M1",
                        "from": {"name": "Sender", "email": "sender@example.test"},
                        "to": [{"name": "Alice", "email": "alice@example.com"}],
                        "subject": ITIP_SUBJECT,
                        "textBody": "(raw_source overrides this)",
                        "receivedAt": "2026-01-02T03:04:07Z",
                        "rawSourceBase64": base64.b64encode(itip_raw).decode("ascii"),
                    },
                ],
            }
        ]
    }
    with open(SEED_PATH, "w", encoding="utf-8") as out:
        json.dump(seed, out)
    return SEED_PATH


def setup():
    write_standalone_mail_account(ACCOUNT)
    say("setup: account sources written, seed file holds the forged message")


def top_level_window_count():
    return evolution().childCount


def run():
    select_folder(ACCOUNT, "Inbox")
    select_message(SUBJECT)
    menu_item("Message/Open in New Window")
    browser = wait_for(lambda: window(SUBJECT), "the detached message window")
    time.sleep(WAIT_SECONDS)
    screenshot("forged-mail-open")

    for expected in (EXPANDER_TEXT, MENU_TEXT, REMOTE_TEXT):
        if find(browser, "push button", expected, showing=True) is None:
            raise ScenarioFailure(f"the forged button {expected!r} never rendered")
    say("mail reader: the forged buttons render")

    # attachment-expander: a wrong-but-plausible value matches no live
    # EAttachment, so the handler's own lookup fails and nothing happens.
    # Safe to click through AT-SPI's doAction: the handler never opens a
    # nested loop when the lookup fails.
    windows_before = top_level_window_count()
    expander = wait_find(None, EXPANDER_TEXT, showing=True)
    do_action(expander)
    time.sleep(2)
    screenshot("after-forged-expander")
    if top_level_window_count() != windows_before:
        raise ScenarioFailure("a forged attachment-expander click opened a window")
    say("attachment-expander: forged click with a wrong value does nothing")

    # attachment-menu: same lookup, so no popup menu appears.
    menu_button = wait_find(None, MENU_TEXT, showing=True)
    do_action(menu_button)
    time.sleep(2)
    screenshot("after-forged-menu")
    if top_level_window_count() != windows_before:
        raise ScenarioFailure("a forged attachment-menu click opened a window")
    say("attachment-menu: forged click with a wrong value opens no menu")

    # __evo-remote-content-img: the handler reads none of the clicked
    # element's data, only the click position, so this is safe to fire
    # through doAction too.
    remote = wait_find(None, REMOTE_TEXT, showing=True)
    do_action(remote)
    time.sleep(2)
    screenshot("after-forged-remote-content")
    say("__evo-remote-content-img: forged click carries no attacker data")

    # The secure-button family: a real OpenPGP-signed message so the
    # listeners actually register (em-format/e-mail-part-secure-button.c's
    # content_loaded, gated on the message carrying a real signed part).
    select_folder(ACCOUNT, "Inbox")
    select_message(SIGNED_SUBJECT)
    menu_item("Message/Open in New Window")
    signed_browser = wait_for(
        lambda: window(SIGNED_SUBJECT), "the detached signed-message window"
    )
    time.sleep(WAIT_SECONDS)
    screenshot("forged-signed-mail-open")

    for expected in (
        SECURE_BUTTON_TEXT,
        SECURE_DETAILS_TEXT,
        SECURE_VIEW_CERT_TEXT,
        SECURE_IMPORT_CERT_TEXT,
    ):
        if find(signed_browser, "push button", expected, showing=True) is None:
            raise ScenarioFailure(f"the forged button {expected!r} never rendered")
    say("signed mail reader: the forged secure-button family renders")

    windows_before = top_level_window_count()

    # secure-button: `value` must prefix-match `g_strdup_printf("%p:",
    # mail_part)`, a real in-memory pointer a static message cannot guess;
    # a wrong-but-plausible value makes the handler return before it does
    # anything (no dialog, no hidden-element toggle).
    secure_button = wait_find(None, SECURE_BUTTON_TEXT, showing=True)
    do_action(secure_button)
    time.sleep(2)
    screenshot("after-forged-secure-button")
    if top_level_window_count() != windows_before:
        raise ScenarioFailure("a forged secure-button click opened a window")
    say("secure-button: forged click with a wrong value does nothing")

    # secure-button-details: same shape, but keyed on the clicked element's
    # own `id`, not `value`.
    secure_details = wait_find(None, SECURE_DETAILS_TEXT, showing=True)
    do_action(secure_details)
    time.sleep(2)
    screenshot("after-forged-secure-button-details")
    if top_level_window_count() != windows_before:
        raise ScenarioFailure("a forged secure-button-details click opened a window")
    say("secure-button-details: forged click with a wrong id does nothing")

    # secure-button-view-certificate / -import-certificate: SMIME-only
    # (`#if defined(ENABLE_SMIME)`) and look up `CamelCipherCertInfo::
    # cert_data`, which a PGP signature's validity never populates, so
    # these are safe either way: unregistered (no SMIME support built) or
    # registered but finding no certificate for any value, real or forged.
    view_cert = wait_find(None, SECURE_VIEW_CERT_TEXT, showing=True)
    do_action(view_cert)
    time.sleep(2)
    screenshot("after-forged-view-certificate")
    if top_level_window_count() != windows_before:
        raise ScenarioFailure("a forged secure-button-view-certificate click opened a window")
    say("secure-button-view-certificate: forged click opens no certificate viewer")

    import_cert = wait_find(None, SECURE_IMPORT_CERT_TEXT, showing=True)
    do_action(import_cert)
    time.sleep(2)
    screenshot("after-forged-import-certificate")
    if top_level_window_count() != windows_before:
        raise ScenarioFailure("a forged secure-button-import-certificate click opened a window")
    say("secure-button-import-certificate: forged click imports nothing")

    # itip-button: a real calendar invitation so the listener registers
    # (em-format/e-mail-part-itip.c's content_loaded calls
    # itip_view_set_web_view unconditionally whenever the part has a
    # message, regardless of whether the invitation parses cleanly).
    select_folder(ACCOUNT, "Inbox")
    select_message(ITIP_SUBJECT)
    menu_item("Message/Open in New Window")
    itip_browser = wait_for(
        lambda: window(ITIP_SUBJECT), "the detached invitation-message window"
    )
    time.sleep(WAIT_SECONDS)
    screenshot("forged-itip-mail-open")

    if find(itip_browser, "push button", ITIP_BUTTON_TEXT, showing=True) is None:
        raise ScenarioFailure(f"the forged button {ITIP_BUTTON_TEXT!r} never rendered")
    say("invitation mail reader: the forged itip-button renders")

    windows_before = top_level_window_count()

    # itip-button: `value` must prefix-match `g_strdup_printf("%p:",
    # view->priv->itip_part_ptr)`, a real in-memory pointer; a
    # plausible-but-wrong value fails the prefix check before any script runs.
    itip_button = wait_find(None, ITIP_BUTTON_TEXT, showing=True)
    do_action(itip_button)
    time.sleep(2)
    screenshot("after-forged-itip-button")
    if top_level_window_count() != windows_before:
        raise ScenarioFailure("a forged itip-button click opened a window")
    say("itip-button: forged click with a wrong value does nothing")


main(setup, run, mock_args=("--seed", write_seed()))
