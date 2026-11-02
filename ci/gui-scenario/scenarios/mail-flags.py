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

Not covered here, left for later claims of the same item: a custom label
with spaces, capitals and non-ASCII (needs driving the "New Label"
dialog), junk/not-junk, a multi-selection, the reverse direction (a
server-side keyword change reflected in the message list), the 3.60.2
Fedora container, and contacts/calendar categories.
"""

from scenario import (
    Jmap,
    expect_log,
    expect_no_errors,
    main,
    menu_item,
    say,
    select_folder,
    select_message,
    wait_for,
    write_standalone_mail_account,
)

ACCOUNT = "JMAP scenario mail"
SUBJECT = "Welcome to jmap-mockd"
KEYWORDS_LOG = 'jmap_mail_sync: setting message keywords .*uid="{}"'
BUILTIN_LABEL_TAGS = {
    "Important": "$Labelimportant",
    "Work": "$Labelwork",
    "Personal": "$Labelpersonal",
    "To Do": "$Labeltodo",
    "Later": "$Labellater",
}


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

    expect_no_errors()


main(setup, run)
