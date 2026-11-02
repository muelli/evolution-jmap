# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scenario: star/unstar, read/unread and labels on one mail message
(ROADMAP item 177(a)).

Message/Mark As/Important and Unimportant toggle the server's `$flagged`
keyword (Evolution's "star"); Message/Mark As/Read and Unread toggle
`$seen`. The mock seeds both messages with no keywords, so the inbox
starts unstarred and unread.

Message/Mark As/Label/<name> toggles a label on; Message/Mark As/Label/
None clears every label. A built-in label is not a plain keyword of its
display name: Evolution's `EMailLabelListStore` hard-codes each one as a
Camel user flag of the literal form `$Label` + the lowercased name with
no space (`e-mail-label-list-store.c`: Important -> `$Labelimportant`,
Work -> `$Labelwork`, Personal -> `$Labelpersonal`, To Do -> `$Labeltodo`,
Later -> `$Labellater`), the same flag Camel's IMAP provider has always
put on the wire for a label, unchanged since long before JMAP support
existed. `jmap-mail::message_info` reads Camel's named user flags
verbatim and `jmap-mail-sync::keywords` treats any non-flag tag as a
label name, so this client writes that same `$Label*` string as the
server's keyword: not cosmetic, but exactly what interop with an IMAP
Evolution on the same account already expects.

Message/Mark As/Junk sets `$junk` (and, same as stock IMAP, `$seen`: Camel's
own handler marks a message read when it calls it junk) after a confirmation
dialog ("Mark as Junk"/"Cancel", `mail.error.xml`'s `ask-mark-as-junk`,
`prompt-on-mark-as-junk` defaults to on). Message/Mark As/Not Junk sets
`$notjunk` with no prompt. Both go through the same `Keywords` machinery as
the flags and labels above (`jmap-mail-sync/src/keywords.rs`'s
`keyword::JUNK`/`NOT_JUNK`).

Message/Mark As/Label/New Label opens the "Add Label" dialog
(`EMailLabelDialog`) and, on OK, applies the new label to the already
selected message right away (`e-mail-reader.c`'s
`action_mail_label_new_cb`). Unlike the five built-in labels, a label made
this way gets no `$Label` prefix: its tag is
`mail_label_list_store_tag_from_name` (`e-mail-label-list-store.c`):
lowercase the ASCII letters only, replace each of ` ()/{%*<>\"` with `_`,
then UTF-7 encode any remaining non-ASCII character (`camel_utf8_utf7`,
IMAP's modified UTF-7, RFC 2152/3501). `CUSTOM_LABEL_TAG` is derived from
that algorithm, cross-checked against EDS's own
`camel/tests/test-camel-utf7.c` vectors.

Message/Mark As/Important acts on the whole selection, not just the
cursor message: with both seeded messages selected at once, one menu click
sets `$flagged` on both.

Not covered here, left for later claims of the same item: a multi-selection
of the other actions (labels, junk, read/unread), the reverse direction (a
server-side keyword change reflected in the message list), the 3.60.2
Fedora container, and contacts/calendar categories.
"""

from scenario import (
    Jmap,
    ScenarioFailure,
    click_button,
    confirm,
    expect_log,
    expect_no_errors,
    find,
    main,
    menu_item,
    say,
    select_folder,
    select_message,
    select_messages,
    text_of,
    wait_for,
    window,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
SUBJECT = "Welcome to jmap-mockd"
SECOND_SUBJECT = "Second message"
KEYWORDS_LOG = 'jmap_mail_sync: setting message keywords .*uid="{}"'
BUILTIN_LABEL_TAGS = {
    "Important": "$Labelimportant",
    "Work": "$Labelwork",
    "Personal": "$Labelpersonal",
    "To Do": "$Labeltodo",
    "Later": "$Labellater",
}
# "Käse Co": a space, two capitals and one non-ASCII character (U+00E4,
# "ä"). Lowercased and space-replaced: "käse_co"; "ä" alone between ASCII
# characters UTF-7 encodes to "&AOQ-" (test-camel-utf7.c's "R\xc3\xa4s" ->
# "R&AOQ-s" vector), giving this exact tag.
CUSTOM_LABEL_NAME = "Käse Co"
CUSTOM_LABEL_TAG = "k&AOQ-se_co"


def set_text_via_atspi(node, content):
    """Set an entry's text through AT-SPI's EditableText interface directly,
    not through scenario.py's set_text (synthetic key events): confirmed
    empirically that generateKeyboardEvent's KEY_STRING form hangs here on a
    non-ASCII character, timing out the whole AT-SPI bus (dbind) rather than
    just failing to change the text, unlike its many ASCII-only uses
    elsewhere. GTK's own EditableText implementation still runs the entry's
    real text-changed path, so this exercises the production "changed"
    handler the same as a real keystroke would.
    """
    node.queryEditableText().setTextContents(content)
    wait_for(lambda: text_of(node) == content, f"the field shows {content!r}")


def setup():
    write_standalone_mail_account(ACCOUNT)
    say("setup: account sources written")


def run():
    server = Jmap()
    message_id = server.emails()[SUBJECT]["id"]

    select_folder(ACCOUNT, "Inbox")
    select_message(SUBJECT)
    say(f"starts unstarred and unread: {server.keywords_of(SUBJECT)!r}")

    menu_item("Message/Mark As/Important")
    wait_for(lambda: "$flagged" in server.keywords_of(SUBJECT), f"{SUBJECT!r} flagged")
    say("server: $flagged set")
    expect_log(KEYWORDS_LOG.format(message_id), "the keyword write for Important")

    menu_item("Message/Mark As/Unimportant")
    wait_for(lambda: "$flagged" not in server.keywords_of(SUBJECT), f"{SUBJECT!r} unflagged")
    say("server: $flagged cleared")

    menu_item("Message/Mark As/Unread")
    wait_for(lambda: "$seen" not in server.keywords_of(SUBJECT), f"{SUBJECT!r} unread")
    say("server: $seen cleared")

    menu_item("Message/Mark As/Read")
    wait_for(lambda: "$seen" in server.keywords_of(SUBJECT), f"{SUBJECT!r} read")
    say("server: $seen set")

    for label, tag in BUILTIN_LABEL_TAGS.items():
        menu_item(f"Message/Mark As/Label/{label}")
        wait_for(lambda tag=tag: tag in server.keywords_of(SUBJECT), f"{SUBJECT!r} has {tag!r}")
        say(f"server: {tag!r} set")

    menu_item("Message/Mark As/Label/None")
    wait_for(
        lambda: not server.keywords_of(SUBJECT) & set(BUILTIN_LABEL_TAGS.values()),
        f"{SUBJECT!r} has no label",
    )
    say("server: all labels cleared")

    menu_item("Message/Mark As/Label/New Label")
    label_dialog = wait_for(lambda: window("Add Label"), "the Add Label dialog")
    # The dialog also holds a GtkColorChooser, itself full of text fields
    # ("Color name:", a hex entry); find the name entry by its own label
    # instead of assuming it is the dialog's only one.
    name_label = wait_for(
        lambda: find(label_dialog, "label", "Label name:"), "the 'Label name:' label"
    )
    entry = name_label.parent.getChildAtIndex(name_label.getIndexInParent() + 1)
    if entry is None or entry.getRoleName() != "text":
        raise ScenarioFailure("no text entry follows the 'Label name:' label")
    set_text_via_atspi(entry, CUSTOM_LABEL_NAME)
    click_button(label_dialog, "OK")
    wait_for(lambda: window("Add Label") is None, "the Add Label dialog closes")
    wait_for(
        lambda: CUSTOM_LABEL_TAG in server.keywords_of(SUBJECT),
        f"{SUBJECT!r} has the custom label {CUSTOM_LABEL_TAG!r}",
    )
    say(f"server: {CUSTOM_LABEL_TAG!r} set from a label name with spaces, capitals and non-ASCII")

    menu_item("Message/Mark As/Label/None")
    wait_for(
        lambda: CUSTOM_LABEL_TAG not in server.keywords_of(SUBJECT),
        f"{SUBJECT!r} custom label cleared",
    )
    say("server: custom label cleared")

    menu_item("Message/Mark As/Junk")
    confirm("Mark as Junk", "mark-as-junk-confirmation")
    wait_for(lambda: "$junk" in server.keywords_of(SUBJECT), f"{SUBJECT!r} junk")
    say("server: $junk set")

    select_message(SUBJECT)
    menu_item("Message/Mark As/Not Junk")
    wait_for(lambda: "$notjunk" in server.keywords_of(SUBJECT), f"{SUBJECT!r} not junk")
    wait_for(lambda: "$junk" not in server.keywords_of(SUBJECT), f"{SUBJECT!r} not junk (cleared)")
    say("server: $notjunk set, $junk cleared")

    select_messages([SUBJECT, SECOND_SUBJECT])
    menu_item("Message/Mark As/Important")
    wait_for(lambda: "$flagged" in server.keywords_of(SUBJECT), f"{SUBJECT!r} flagged")
    wait_for(lambda: "$flagged" in server.keywords_of(SECOND_SUBJECT), f"{SECOND_SUBJECT!r} flagged")
    say("server: $flagged set on both selected messages")

    menu_item("Message/Mark As/Unimportant")
    wait_for(lambda: "$flagged" not in server.keywords_of(SUBJECT), f"{SUBJECT!r} unflagged")
    wait_for(
        lambda: "$flagged" not in server.keywords_of(SECOND_SUBJECT),
        f"{SECOND_SUBJECT!r} unflagged",
    )
    say("server: $flagged cleared on both selected messages")

    expect_no_errors()


main(setup, run)
