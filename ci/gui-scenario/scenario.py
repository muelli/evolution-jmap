# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Shared helpers of the GUI scenarios.

ci/gui-scenario/run.sh calls each scenario two times: "setup" before
Evolution starts, "run" after. The helpers here start no process. They find
and operate AT-SPI nodes with timeouts, query the server through JMAP, write
the source files, read the Evolution log and take screenshots.
"""

import json
import os
import re
import subprocess
import sys
import time
import urllib.request

ARTIFACTS = os.environ.get("GUI_SCENARIO_ARTIFACTS", ".")
JMAP_URL = os.environ.get("GUI_SCENARIO_JMAP_URL", "http://127.0.0.1:8090")
MOCK_PORT = os.environ.get("GUI_SCENARIO_MOCK_PORT", "8090")
SOURCES = os.environ.get("GUI_SCENARIO_SOURCES", ".")
# ci/dovecot-oracle.sh's own container name and port, forwarded in by
# ci/gui-scenario/run.sh when it joined the oracle's podman network. Empty
# when no oracle is running; a scenario that needs it fails fast instead.
IMAP_HOST = os.environ.get("GUI_SCENARIO_IMAP_HOST", "")
IMAP_PORT = os.environ.get("GUI_SCENARIO_IMAP_PORT", "")
DEFAULT_TIMEOUT = 60
POLL_SECONDS = 0.5
# The X keysym of the Menu key.
MENU_KEY = 0xFF67

CORE_CAPABILITY = "urn:ietf:params:jmap:core"
MAIL_CAPABILITY = "urn:ietf:params:jmap:mail"
CONTACTS_CAPABILITY = "urn:ietf:params:jmap:contacts"
CALENDARS_CAPABILITY = "urn:ietf:params:jmap:calendars"
SUBMISSION_CAPABILITY = "urn:ietf:params:jmap:submission"


class ScenarioFailure(Exception):
    """A step did not reach its expected state."""


def say(text):
    print(f"[{time.strftime('%H:%M:%S')}] {text}", flush=True)


def wait_for(probe, what, timeout=DEFAULT_TIMEOUT):
    """Call `probe` until it returns a value that is not None or False.

    Raise ScenarioFailure with `what` if the timeout ends first. An exception
    in `probe` counts as "not yet": AT-SPI nodes disappear while GTK rebuilds
    a view.
    """
    deadline = time.monotonic() + timeout
    last_error = None
    while True:
        try:
            value = probe()
            if value is not None and value is not False:
                return value
        except Exception as error:  # noqa: BLE001
            last_error = error
        if time.monotonic() >= deadline:
            detail = f" (last error: {last_error})" if last_error else ""
            raise ScenarioFailure(f"timed out after {timeout}s: {what}{detail}")
        time.sleep(POLL_SECONDS)


# Screenshots.

_shot_number = 0


def screenshot(label):
    """Save the screen as NN-label.png in the artifacts directory."""
    global _shot_number
    _shot_number += 1
    path = os.path.join(ARTIFACTS, f"{_shot_number:02d}-{label}.png")
    subprocess.run(["import", "-window", "root", path], check=False)
    say(f"screenshot {os.path.basename(path)}")
    return path


# Source files.


def write_source(uid, text):
    with open(os.path.join(SOURCES, f"{uid}.source"), "w", encoding="utf-8") as out:
        out.write(text)


def write_standalone_mail_account(
    name="JMAP scenario mail", key="scenario", identity=("Alice", "alice@example.com")
):
    """Write a standalone mail account, its identity and its transport.

    The account uses the mock without credentials. `key` makes the source
    uids unique, so that a scenario can write more than one account.
    `identity` is the (name, address) of the sender. Return the display name.
    """
    connection = f"""[Authentication]
Host=127.0.0.1
Port={MOCK_PORT}

[Security]
Method=none
"""
    write_source(
        f"{key}-mail",
        f"""[Data Source]
DisplayName={name}
Enabled=true

[Mail Account]
BackendName=jmap
IdentityUid={key}-identity

{connection}""",
    )
    sender, address = identity
    write_source(
        f"{key}-identity",
        f"""[Data Source]
DisplayName={sender}
Enabled=true

[Mail Identity]
Name={sender}
Address={address}

[Mail Submission]
TransportUid={key}-transport
""",
    )
    write_source(
        f"{key}-transport",
        f"""[Data Source]
DisplayName={name} transport
Enabled=true

[Mail Transport]
BackendName=jmap

{connection}""",
    )
    return name


def write_standalone_imap_account(
    name="IMAP scenario mail",
    key="imap-scenario",
    user="tester",
    identity=("Alice", "alice@example.com"),
):
    """Write a standalone IMAP account and its identity, against
    ci/dovecot-oracle.sh's own container.

    `IMAP_HOST`/`IMAP_PORT` (ci/gui-scenario/run.sh's forwarded
    `GUI_SCENARIO_IMAP_HOST`/`_PORT`) must already name the oracle: start it
    first with `ci/dovecot-oracle.sh start`, on the same podman network this
    script's own container joins. No `[Authentication] Method`: the empty
    default (`camel_imapx_password_authtype`'s own `authproto`, `""`) is a
    plain IMAP `LOGIN`, matching the oracle's `static`/`nopassword=y` passdb,
    which accepts any password, including the empty one
    `dismiss_first_run_dialogs` submits (there is no keyring here to cache
    one). Return the display name.
    """
    if not IMAP_HOST or not IMAP_PORT:
        raise ScenarioFailure(
            "no IMAP oracle: start one first with 'ci/dovecot-oracle.sh start', "
            "then rerun ci/gui-scenario/run.sh so it joins the oracle's network"
        )
    write_source(
        f"{key}-mail",
        f"""[Data Source]
DisplayName={name}
Enabled=true

[Mail Account]
BackendName=imapx
IdentityUid={key}-identity

[Authentication]
Host={IMAP_HOST}
Port={IMAP_PORT}
User={user}

[Security]
Method=none
""",
    )
    sender, address = identity
    write_source(
        f"{key}-identity",
        f"""[Data Source]
DisplayName={sender}
Enabled=true

[Mail Identity]
Name={sender}
Address={address}
""",
    )
    return name


def write_collection_mail_account(name="JMAP scenario mail", identity=("Alice", "alice@example.com")):
    """Write a mail account in a collection, as the account assistant makes it.

    The collection holds the server and the address. The mail account, the
    identity and the transport are its children, without a server of their
    own. The identity
    has the account name: the account editor names the account after it.
    Return the display name.
    """
    write_source(
        "collection-mail",
        f"""[Data Source]
DisplayName={name}
Enabled=true

[Collection]
BackendName=jmap
Identity={identity[1]}
ContactsEnabled=false
CalendarEnabled=false
MailEnabled=true

[Authentication]
Host=127.0.0.1
Port={MOCK_PORT}

[Security]
Method=none
""",
    )
    sender, address = identity
    write_source(
        "collection-mail-account",
        f"""[Data Source]
DisplayName={name}
Enabled=true
Parent=collection-mail

[Mail Account]
BackendName=jmap
IdentityUid=collection-mail-identity
""",
    )
    write_source(
        "collection-mail-identity",
        f"""[Data Source]
DisplayName={name}
Enabled=true
Parent=collection-mail

[Mail Identity]
Name={sender}
Address={address}

[Mail Submission]
TransportUid=collection-mail-transport
""",
    )
    write_source(
        "collection-mail-transport",
        f"""[Data Source]
DisplayName={name}
Enabled=true
Parent=collection-mail

[Mail Transport]
BackendName=jmap
""",
    )
    return name


def write_collection_account(name="JMAP scenario collection"):
    """Write a collection account with contacts and calendars, without mail.

    The account uses the mock without credentials. Return the display name.
    """
    write_source(
        "scenario-collection",
        f"""[Data Source]
DisplayName={name}
Enabled=true

[Collection]
BackendName=jmap
ContactsEnabled=true
CalendarEnabled=true
MailEnabled=false

[Authentication]
Host=127.0.0.1
Port={MOCK_PORT}

[Security]
Method=none
""",
    )
    return name


# JMAP.


class Jmap:
    """A minimal JMAP client for server-state checks. It sends no credentials.

    The calls go to `account_id`, by default the primary mail account.
    """

    def __init__(self, url=JMAP_URL, account_id=None):
        self.url = url
        session = wait_for(self._session, f"JMAP session at {url}", timeout=30)
        self.api_url = session["apiUrl"]
        self.account_id = account_id or session["primaryAccounts"][MAIL_CAPABILITY]

    def _session(self):
        with urllib.request.urlopen(f"{self.url}/.well-known/jmap", timeout=5) as response:
            return json.load(response)

    def call(self, method, arguments, extra_using=()):
        """Send one method call and return its response arguments.

        `extra_using` adds capabilities to the standard ones of the request.
        """
        arguments = {"accountId": self.account_id, **arguments}
        body = json.dumps(
            {
                "using": [
                    CORE_CAPABILITY,
                    MAIL_CAPABILITY,
                    CONTACTS_CAPABILITY,
                    CALENDARS_CAPABILITY,
                    SUBMISSION_CAPABILITY,
                    *extra_using,
                ],
                "methodCalls": [[method, arguments, "0"]],
            }
        ).encode()
        request = urllib.request.Request(
            self.api_url, data=body, headers={"Content-Type": "application/json"}
        )
        with urllib.request.urlopen(request, timeout=10) as response:
            name, result, _ = json.load(response)["methodResponses"][0]
        if name == "error":
            raise ScenarioFailure(f"{method} failed: {result}")
        return result

    def create(self, type_name, properties):
        """Create one object of `type_name` ("Calendar", ...) and return its id."""
        created = self.call(f"{type_name}/set", {"create": {"c": properties}})
        if "c" not in (created.get("created") or {}):
            raise ScenarioFailure(f"{type_name}/set did not create {properties}: {created}")
        return created["created"]["c"]["id"]

    def update(self, type_name, object_id, patch):
        """Patch one object of `type_name` with `patch` (RFC 8620 §5.3)."""
        updated = self.call(f"{type_name}/set", {"update": {object_id: patch}})
        if object_id not in (updated.get("updated") or {}):
            raise ScenarioFailure(f"{type_name}/set did not update {object_id}: {updated}")

    def get(self, type_name, object_id):
        """Return one object of `type_name`, or None if the server has no such object."""
        found = self.call(f"{type_name}/get", {"ids": [object_id]})["list"]
        return found[0] if found else None

    def names(self, type_name):
        """Return {id: name} of all objects of `type_name` ("Calendar", ...)."""
        found = self.call(f"{type_name}/get", {"ids": None})["list"]
        return {item["id"]: item.get("name") for item in found}

    def mailboxes(self):
        """Return {name: mailbox} of all mailboxes."""
        mailboxes = self.call("Mailbox/get", {"ids": None})["list"]
        return {mailbox["name"]: mailbox for mailbox in mailboxes}

    def mailbox_id_or_none(self, role):
        """Return the id of the mailbox with `role`, or None."""
        for mailbox in self.mailboxes().values():
            if mailbox.get("role") == role:
                return mailbox["id"]
        return None

    def mailbox_id(self, role):
        mailbox_id = self.mailbox_id_or_none(role)
        if mailbox_id is None:
            raise ScenarioFailure(f"the server has no mailbox with the role {role!r}")
        return mailbox_id

    def create_mailbox(self, name, role=None):
        created = self.call("Mailbox/set", {"create": {"m": {"name": name, "role": role}}})
        if "m" not in (created.get("created") or {}):
            raise ScenarioFailure(f"Mailbox/set did not create {name!r}: {created}")
        return created["created"]["m"]["id"]

    def emails(self):
        """Return {subject: email} of all messages, with their mailboxIds and keywords."""
        ids = self.call("Email/query", {})["ids"]
        if not ids:
            return {}
        found = self.call(
            "Email/get", {"ids": ids, "properties": ["subject", "mailboxIds", "keywords"]}
        )
        return {email["subject"]: email for email in found["list"]}

    def submissions(self):
        """Return all EmailSubmission objects of the account."""
        return self.call("EmailSubmission/get", {"ids": None})["list"]

    def identities(self):
        """Return {email: identity} of all identities of the account."""
        found = self.call("Identity/get", {"ids": None})["list"]
        return {identity["email"]: identity for identity in found}

    def mailbox_of(self, subject):
        """Return the set of mailbox ids of a message, or None if it is gone."""
        email = self.emails().get(subject)
        return None if email is None else set(email["mailboxIds"])

    def keywords_of(self, subject):
        """Return the set of keyword names set on a message, or None if it is gone."""
        email = self.emails().get(subject)
        if email is None:
            return None
        return {name for name, value in (email.get("keywords") or {}).items() if value}


# AT-SPI.

try:
    import pyatspi
except ImportError:  # The setup step runs without AT-SPI.
    pyatspi = None


def _name(node):
    try:
        return node.name or ""
    except Exception:  # noqa: BLE001
        return ""


def _role(node):
    try:
        return node.getRoleName()
    except Exception:  # noqa: BLE001
        return ""


def descendants(node, role=None, max_depth=30):
    """Yield the descendants of `node` (itself first), depth first."""
    if node is None or max_depth < 0:
        return
    if role is None or _role(node) == role:
        yield node
    try:
        count = node.childCount
    except Exception:  # noqa: BLE001
        return
    for i in range(count):
        try:
            child = node.getChildAtIndex(i)
        except Exception:  # noqa: BLE001
            continue
        yield from descendants(child, role, max_depth - 1)


def find(node, role=None, name=None, prefix=None, showing=False):
    """Return the first descendant that matches, or None."""
    for candidate in descendants(node, role):
        text = _name(candidate)
        if name is not None and text != name:
            continue
        if prefix is not None and not text.startswith(prefix):
            continue
        if showing and not is_showing(candidate):
            continue
        return candidate
    return None


def is_showing(node):
    try:
        return node.getState().contains(pyatspi.STATE_SHOWING)
    except Exception:  # noqa: BLE001
        return False


# Focusable-widget accessibility audit (item 174(d)).

# Roles a screen reader can land keyboard focus on. Labels, static text and
# plain containers are excluded: they are not focusable themselves. A
# GtkTreeView reports as "table" over AT-SPI (confirmed in a dump of the
# Server Filters page), not "tree table". "menu item" is a leaf of an open
# menu; the menu itself (role "menu") is not in this set, since arrow-key
# navigation lands on its items, not on the menu container.
FOCUSABLE_ROLES = {"push button", "check box", "text", "combo box", "table", "menu item"}


def labelled_by_name(node):
    """Return the name of the first labelled-by relation target, or "".

    GTK sets this relation automatically for a field whose caption used
    `gtk_label_set_mnemonic_widget`: a screen reader resolves the field's
    name through it even though the field's own accessible name stays
    empty, so this counts as named too.
    """
    try:
        relations = node.getRelationSet()
    except Exception:  # noqa: BLE001
        return ""
    for relation in relations:
        if relation.getRelationType() != pyatspi.RELATION_LABELLED_BY:
            continue
        for i in range(relation.getNTargets()):
            text = _name(relation.getTarget(i))
            if text:
                return text
    return ""


def unnamed_widgets(root):
    """Yield the role of every focusable descendant of `root` with no
    accessible name and no labelled-by relation to a named widget.

    Skips a node that is not `STATE_SHOWING`: an `EDateEdit` with
    `show_time` off keeps its hidden time combo (and that combo's own
    editable entry) in the AT-SPI tree, state VISIBLE but not SHOWING, the
    same way a notebook page that is not the selected one keeps its
    widgets in the tree with SHOWING unset. Tab can never reach a hidden
    widget, so it is not a reachable "focusable widget with no name" in
    the sense this scenario checks.
    """
    for node in descendants(root):
        if _role(node) not in FOCUSABLE_ROLES:
            continue
        if not node.getState().contains(pyatspi.STATE_SHOWING):
            continue
        if _name(node) or labelled_by_name(node):
            continue
        yield _role(node)


def audit_dialog(dialog, label):
    """Raise if a focusable descendant of `dialog` has no accessible name."""
    offenders = list(unnamed_widgets(dialog))
    if offenders:
        raise ScenarioFailure(f"unnamed focusable widgets in the {label!r} dialog: {offenders}")
    say(f"a11y-audit: no unnamed focusable widget in the {label!r} dialog")


def mnemonic_of(node):
    """Return the lowercase Alt+letter mnemonic AT-SPI reports for `node`, or "".

    GTK exposes a widget's effective mnemonic, its own underlined letter, or
    the one of a label that targets it through `gtk_label_set_mnemonic_
    widget`, as an Action key binding such as "<Alt>e". A widget with no
    mnemonic has no such binding; a plain `GtkLabel` exposes no Action
    interface at all (confirmed empirically: one never turns up in this
    walk), so it never needs excluding by role.
    """
    try:
        action = node.queryAction()
    except Exception:  # noqa: BLE001
        return ""
    prefix = "<Alt>"
    for i in range(action.nActions):
        binding = action.getKeyBinding(i) or ""
        if binding.startswith(prefix):
            return binding[len(prefix) :].lower()
    return ""


def duplicate_mnemonics(root):
    """Yield (letter, [names]) for each Alt+letter mnemonic two or more
    showing descendants of `root` share.

    Scope: one real window or dialog, where Alt+letter is a direct keyboard
    accelerator, and a clash leaves one of the two widgets unreachable by
    it. A popup menu is not walked with this: GTK lets several menu items
    share one underlined letter and cycles through them while the menu is
    open, a different mechanism with no unreachable widget, so it is not
    the failure mode this check is for.
    """
    by_letter = {}
    for node in descendants(root):
        if not node.getState().contains(pyatspi.STATE_SHOWING):
            continue
        letter = mnemonic_of(node)
        if not letter:
            continue
        by_letter.setdefault(letter, []).append(_name(node) or f"<{_role(node)}>")
    for letter, names in by_letter.items():
        if len(names) > 1:
            yield letter, names


def audit_mnemonics(root, label):
    """Raise if two or more showing widgets under `root` share one Alt+letter mnemonic."""
    clashes = list(duplicate_mnemonics(root))
    if clashes:
        detail = "; ".join(f"Alt+{letter.upper()} on {names}" for letter, names in clashes)
        raise ScenarioFailure(f"duplicate mnemonics in the {label!r} window: {detail}")
    say(f"a11y-audit: no duplicate mnemonic in the {label!r} window")


# The X keysym of the Tab key.
TAB_KEY = 0xFF09

# Roles whose AT-SPI accessible reliably reports STATE_FOCUSED while it
# holds keyboard focus. "combo box" is excluded: confirmed empirically
# (gui-scenario container, the rule dialog's condition/action comboboxes)
# that tabbing onto a plain GtkComboBox produces no STATE_FOCUSED node
# anywhere under the window for the whole dwell time, even though Tab
# still reaches the widget after it on schedule, so the focus ring itself
# is intact and ATK simply never surfaces it. "menu item" is excluded for
# the same reason FOCUSABLE_ROLES documents it stays out of the mnemonic
# walk: it is a popup leaf, not a window-level Tab stop.
TAB_OBSERVABLE_ROLES = {"push button", "check box", "text", "table"}


def focused_node(root):
    """Return the descendant of `root` with STATE_FOCUSED, or None."""
    for node in descendants(root):
        try:
            if node.getState().contains(pyatspi.STATE_FOCUSED):
                return node
        except Exception:  # noqa: BLE001
            continue
    return None


def _tab_key(node):
    return (_name(node) or labelled_by_name(node), _role(node))


def _wait_focus_change(root, timeout_polls=6):
    """Poll for a STATE_FOCUSED node under `root`, or return None."""
    node = None
    for _poll in range(timeout_polls):
        node = focused_node(root)
        if node is not None:
            break
        time.sleep(POLL_SECONDS)
    return node


def tab_reachable(root, max_presses=120):
    """Press Tab repeatedly from the widget that already has focus and
    return the (name, role) pairs AT-SPI reports STATE_FOCUSED for along
    the way, stopping once the chain returns to the start.

    The caller must already have given keyboard focus to some widget
    inside `root` (opening a page or a dialog always leaves one focused).
    A step that lands on a widget outside TAB_OBSERVABLE_ROLES (a combo
    box) produces no observable STATE_FOCUSED node for a few polls; that
    is expected and skipped, not a failure, as long as the chain still
    closes within `max_presses`.

    Not suitable for a window that holds a STATE_MULTI_LINE text view with
    GTK's own `accepts-tab` default (TRUE): a plain Tab there inserts a tab
    character instead of moving focus, and this container has no working
    way to synthesize the Ctrl+Tab that would normally escape it (AT-SPI's
    keyboard event generator takes a modifier only as a keysym *string* on
    a KEY_PRESS/KEY_RELEASE pair, which left Control unobserved as held by
    the following Tab in every form tried). The Vacation Responder page's
    message body set `accepts-tab` FALSE (item 174(b)) and no longer needs
    this exclusion; callers with such a surface skip this check; see
    a11y-audit.py's docstring.
    """
    start = wait_for(lambda: focused_node(root), "a focused widget to start the Tab walk")
    start_key = _tab_key(start)
    reached = {start_key}
    for _ in range(max_presses):
        pyatspi.Registry.generateKeyboardEvent(TAB_KEY, None, pyatspi.KEY_SYM)
        node = _wait_focus_change(root)
        if node is None:
            continue
        key = _tab_key(node)
        if key == start_key:
            return reached
        reached.add(key)
    raise ScenarioFailure(f"Tab did not return to the start within {max_presses} presses")


def audit_tab_reachability(root, label, max_presses=120):
    """Raise if a showing, sensitive widget under `root`, of a role whose
    focus AT-SPI can observe (TAB_OBSERVABLE_ROLES), is never visited while
    repeatedly pressing Tab from whatever already has focus.

    Expects STATE_SHOWING and STATE_SENSITIVE, not STATE_FOCUSABLE:
    confirmed empirically (gui-scenario container, the Server Filters
    page's selection-dependent buttons) that a disabled `GtkButton` keeps
    reporting STATE_FOCUSABLE regardless, the opposite of what
    STATE_FOCUSABLE means for a check box, combo box or text entry, where
    it does track sensitivity. GTK's own Tab order skips an insensitive
    widget of any role, so STATE_SENSITIVE is the one check that holds
    for all of them.

    Retries the walk once on failure: a synthetic Tab key event is
    occasionally dropped under Xvfb (confirmed empirically: an identical
    walk that failed this way passed cleanly on an immediate rerun), so
    one retry absorbs a single dropped event before this counts as a
    real finding instead of a flake.
    """
    try:
        reached = tab_reachable(root, max_presses)
    except ScenarioFailure:
        reached = tab_reachable(root, max_presses)
    missing = []
    for node in descendants(root):
        if _role(node) not in TAB_OBSERVABLE_ROLES:
            continue
        state = node.getState()
        if not state.contains(pyatspi.STATE_SHOWING) or not state.contains(pyatspi.STATE_SENSITIVE):
            continue
        key = _tab_key(node)
        if key not in reached:
            missing.append(key)
    if missing:
        raise ScenarioFailure(f"Tab never reaches {missing} in the {label!r} window")
    say(f"a11y-audit: Tab reaches every observable focusable widget in the {label!r} window")


def evolution():
    """Return the Evolution application node, or None."""
    desktop = pyatspi.Registry.getDesktop(0)
    for i in range(desktop.childCount):
        app = desktop.getChildAtIndex(i)
        if app is not None and _name(app) in ("evolution", "org.gnome.Evolution"):
            return app
    return None


def wait_find(role=None, name=None, prefix=None, root=None, showing=False, timeout=DEFAULT_TIMEOUT):
    """Wait for a node under `root` (default: Evolution) and return it."""
    what = f"{role or 'node'} {name or prefix or ''!r}"
    return wait_for(
        lambda: find(root or evolution(), role, name, prefix, showing),
        what,
        timeout,
    )


def do_action(node, action_name=None):
    """Run the named action of `node`, or its first action."""
    action = node.queryAction()
    index = 0
    if action_name is not None:
        names = [action.getName(i) for i in range(action.nActions)]
        if action_name not in names:
            raise ScenarioFailure(f"{_name(node)!r} has no action {action_name!r}: {names}")
        index = names.index(action_name)
    action.doAction(index)


def window(prefix):
    """Return the showing top-level window of Evolution whose name starts with `prefix`, or None."""
    app = evolution()
    for i in range(app.childCount):
        candidate = app.getChildAtIndex(i)
        if _name(candidate).startswith(prefix) and is_showing(candidate):
            return candidate
    return None


def click_at(node):
    """Click `node` with a synthetic mouse event, not an AT-SPI action call.

    A button whose handler opens a modal dialog (`gtk_dialog_run`) leaves
    the application unreachable over AT-SPI for the rest of an in-flight
    `doAction` call: the action dispatch and the nested main loop it starts
    share the same call stack. A real click does not have this problem, so
    a button that opens a dialog uses this instead of `do_action`.
    """
    extents = node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
    x = extents.x + extents.width // 2
    y = extents.y + extents.height // 2
    pyatspi.Registry.generateMouseEvent(x, y, "b1c")


def double_click(node):
    """Double-click the centre of `node` with the synthetic mouse."""
    extents = node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
    x = extents.x + extents.width // 2
    y = extents.y + extents.height // 2
    pyatspi.Registry.generateMouseEvent(x, y, "b1d")


def right_click(node):
    """Right-click the centre of `node` with the synthetic mouse, to open its context menu."""
    extents = node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
    x = extents.x + extents.width // 2
    y = extents.y + extents.height // 2
    pyatspi.Registry.generateMouseEvent(x, y, "b3c")


def text_of(node):
    """Return the full text of a text node."""
    text = node.queryText()
    return text.getText(0, text.characterCount)


def text_field(root, content):
    """Return the text node under `root` that holds exactly `content`, or None.

    The fields of the editors have no accessible name, so find them by content.
    """
    for candidate in descendants(root, "text"):
        try:
            if text_of(candidate) == content:
                return candidate
        except Exception:  # noqa: BLE001
            continue
    return None


def labelled_field(root, label):
    """Return the widget that comes just before the label `label` under `root`, or None.

    The composer header table has no label relations for some fields, but each
    field is the sibling just before its label.
    """
    for node in descendants(root, "label"):
        if _name(node) != label:
            continue
        parent = node.parent
        index = node.getIndexInParent()
        if parent is not None and index > 0:
            return parent.getChildAtIndex(index - 1)
    return None


def set_text(node, content):
    """Replace the text of an editable text node through the keyboard.

    The contact editor ignores a text that AT-SPI sets: its Save stays
    disabled. Key events count as an edit in each editor.
    """
    node.queryComponent().grabFocus()
    node.queryEditableText().deleteText(0, len(text_of(node)))
    pyatspi.Registry.generateKeyboardEvent(0, content, pyatspi.KEY_STRING)
    wait_for(lambda: text_of(node) == content, f"the field shows {content!r}")
    say(f"typed {content!r}")


MENU_ITEM_ROLES = ("menu item", "check menu item", "radio menu item")


def _menu_entry(root, path):
    """Return the menu item at `path` ("File/Send Later/In One Hour") under `root`, or None.

    A menu bar can have two menus with the same name (the composer has an
    empty "File" menu first), so each match is tried. The terminal item can
    be a plain, check or radio menu item: Evolution's label menu, for
    example, has one check menu item per label.
    """
    first, _, rest = path.partition("/")
    if not rest:
        for role in MENU_ITEM_ROLES:
            entry = find(root, role, first)
            if entry is not None:
                return entry
        return None
    for menu in descendants(root, "menu"):
        if _name(menu) == first:
            entry = _menu_entry(menu, rest)
            if entry is not None:
                return entry
    return None


def menu_item(path, window_prefix=None):
    """Activate a menu item. `path` is like "Folder/Expunge" or "File/New/Mail Message".

    The menu bar is the one of the main window, or of the window whose name
    starts with `window_prefix`. The item is found by name inside its menus,
    so the menus need not open. Return after the action starts, not after its
    effect.
    """

    def root():
        return window(window_prefix) if window_prefix else evolution()

    entry = wait_for(lambda: _menu_entry(root(), path), f"menu item {path!r}")
    wait_for(lambda: entry.getState().contains(pyatspi.STATE_SENSITIVE), f"{path!r} sensitive")
    # Some actions run a modal dialog inside the doAction call, and the call
    # then returns only when the dialog closes. A child process makes the call,
    # so that this process can operate the dialog.
    subprocess.Popen([sys.executable, __file__, "menu-item", path, window_prefix or ""])
    say(f"menu {path}")


def _activate_menu_item(path, window_prefix):
    """The child process of menu_item."""
    root = window(window_prefix) if window_prefix else evolution()
    do_action(_menu_entry(root, path))


def click_button(root, name, timeout=DEFAULT_TIMEOUT):
    """Click the button `name` under `root`.

    Match by name only: the role is "push button" in some AT-SPI versions and
    "button" in others.
    """
    button = wait_find(None, name, root=root, timeout=timeout)
    do_action(button)
    say(f"clicked {name!r}")


def confirm(button, label=None):
    """Wait for a dialog with the button `button`, then click that button.

    Evolution's confirmation dialogs have no useful name, so find them by the
    button. With `label`, save a screenshot of the dialog first.
    """

    def dialog_with_button():
        app = evolution()
        for i in range(app.childCount):
            window = app.getChildAtIndex(i)
            if _role(window) in ("dialog", "alert") and find(window, None, button):
                return window
        return None

    dialog = wait_for(dialog_with_button, f"a dialog with the button {button!r}")
    if label is not None:
        screenshot(label)
    click_button(dialog, button)


def dismiss_first_run_dialogs():
    """Click away the dialogs of a new profile, when they show.

    The mail authentication request: clear "remember password" (there is no
    keyring) and click OK with empty fields. See docs/gui-smoke-test.md.
    """
    app = evolution()
    if app is None:
        return
    dialog = find(app, "dialog", "Mail authentication request")
    if dialog is not None:
        checkbox = find(dialog, "check box")
        if checkbox is not None and checkbox.getState().contains(pyatspi.STATE_CHECKED):
            do_action(checkbox)
        ok = find(dialog, None, "OK")
        if ok is not None:
            do_action(ok)
            say("dismissed the mail authentication request")
    later = find(app, None, "Do not change settings")
    if later is not None:
        do_action(later)


# The mail view.


def folder_tree():
    return wait_find("tree table", "Mail Folder Tree")


def folder_cells(account):
    """Return the folder tree cells of `account`, in tree order.

    AT-SPI gives no parent link between the cells of a tree table. This
    function returns the cells between the cell of the account and the cell
    of the next account.
    """
    cells = [cell for cell in descendants(folder_tree(), "table cell") if _name(cell)]
    names = [_name(cell) for cell in cells]
    if account not in names:
        return []
    start = names.index(account) + 1
    # Stop at the next account. Its folders must not match while this account
    # still loads.
    others = _account_names() - {account}
    end = next((i for i in range(start, len(names)) if names[i] in others), len(names))
    return cells[start:end]


def _account_names():
    """Return the names of the mail accounts in the folder tree."""
    names = {"On This Computer", "Search Folders", "News and Blogs"}
    for entry in os.listdir(SOURCES):
        with open(os.path.join(SOURCES, entry), encoding="utf-8") as source:
            text = source.read()
        match = re.search(r"^DisplayName=(.*)$", text, re.MULTILINE)
        if match and "[Mail Account]" in text:
            names.add(match.group(1))
    return names


def folder_cell(account, folder):
    """Return the tree cell of `folder` in `account`, or None.

    The cell name is the folder name, or "Name (N)" with N unread messages.
    """
    for cell in folder_cells(account):
        text = _name(cell)
        if text == folder or text.startswith(f"{folder} ("):
            return cell
    return None


def select_folder(account, folder):
    """Select a folder in the folder tree and wait until the window shows it."""
    cell = wait_for(
        lambda: (dismiss_first_run_dialogs(), folder_cell(account, folder))[1],
        f"folder {folder!r} of {account!r} in the folder tree",
    )
    # "activate" opens the folder but leaves the tree without a selection, and
    # Evolution then disables the folder actions. Focus moves the tree cursor,
    # and the cursor selects the row.
    cell.queryComponent().grabFocus()
    # A store that connects again asks for the password again.
    wait_for(
        lambda: (dismiss_first_run_dialogs(), _title_shows(folder))[1],
        f"the main window shows {folder!r}",
    )
    say(f"selected folder {folder!r}")


def _title_shows(folder):
    """The window title is the folder name, then the unread count if any."""
    for frame in descendants(evolution(), "frame", max_depth=1):
        title = _name(frame)
        if title == folder or title.startswith(f"{folder} ("):
            return True
    return False


def message_list():
    return wait_find("tree table", "Messages", showing=True)


def popup_menu(subject):
    """Open the context menu of the message with `subject` with the Menu key.

    A right-click does not work: the AT-SPI extents of an ETable cell point
    at the column header.
    """
    select_message(subject)
    pyatspi.Registry.generateKeyboardEvent(MENU_KEY, None, pyatspi.KEY_SYM)
    say(f"opened the context menu of {subject!r}")


def select_message(subject):
    """Select the message with `subject` in the message list."""
    cell = wait_for(
        lambda: find(message_list(), "table cell", subject),
        f"message {subject!r} in the message list",
    )
    cell.queryComponent().grabFocus()
    do_action(cell)
    say(f"selected message {subject!r}")


def select_messages(subjects):
    """Select every message in `subjects` at once, a multi-row selection.

    Uses the table's own `addRowSelection`, not a held Ctrl key: a synthetic
    modifier held across a separate mouse or key event is not reliable in
    this AT-SPI/Xvfb setup (the same reason `tab_reachable` cannot
    synthesize Ctrl+Tab). `addRowSelection` reaches the identical
    `GtkTreeSelection` a Ctrl+click would, so a menu action that follows
    still sees and acts on the real multi-row selection.
    """
    select_message(subjects[0])
    table = message_list()
    rows = table.queryTable()
    for subject in subjects:
        cell = wait_for(
            lambda subject=subject: find(table, "table cell", subject),
            f"message {subject!r} in the message list",
        )
        row = rows.getRowAtIndex(cell.getIndexInParent())
        if not rows.addRowSelection(row):
            raise ScenarioFailure(f"could not add message {subject!r} (row {row}) to the selection")
    say(f"selected messages {list(subjects)!r}")


# The message list's default column order (message-list.etspec's
# <ETableState>): Status, Attachment, Flagged, From, Subject, Date.
STATUS_COLUMN = 0
FLAGGED_COLUMN = 2


def message_row_cell(subject, column):
    """Return the table cell at `column` in the row of the message `subject`.

    `STATUS_COLUMN` and `FLAGGED_COLUMN` are the two columns with a visible
    AT-SPI signal: both are `ECellToggle`s, so AT-SPI reports a value-derived
    name or state, not a pixbuf. A label's row colour has neither: it is a
    `GdkRGBA` painted straight onto the row's GnomeCanvas item
    (`message-list.c`'s `ml_get_cell_color`), so no AT-SPI property carries
    it.
    """
    table = message_list()
    rows = table.queryTable()
    cell = wait_for(
        lambda: find(table, "table cell", subject),
        f"message {subject!r} in the message list",
    )
    row = rows.getRowAtIndex(cell.getIndexInParent())
    return rows.getAccessibleAt(row, column)


def message_is_flagged(subject):
    """Whether the message `subject` shows the star icon (Flagged column).

    `gal-a11y-e-cell-toggle.c`'s `update_cell` sets `ATK_STATE_CHECKED` on a
    toggle cell whose `ECellToggle` value is non-zero and has no configured
    icon description; the Flagged column's cell never gets one
    (`message-list.c` never calls `e_cell_toggle_set_icon_descriptions` for
    its `render_flagged` cell), so checked state is the only AT-SPI signal.
    """
    return message_row_cell(subject, FLAGGED_COLUMN).getState().contains(pyatspi.STATE_CHECKED)


def message_status_name(subject):
    """Return the Status column cell's name: "Unseen" or "Seen".

    The Status column's `render_message_status` cell does get
    `e_cell_toggle_set_icon_descriptions` (`status_map`), so
    `gal_a11y_e_cell_toggle`'s `update_cell` sets this string as the cell's
    AT-SPI name instead of a checked state.
    """
    return message_row_cell(subject, STATUS_COLUMN).name


# The calendar and contacts views.


def source_cell(name):
    """Return the showing cell of the source `name` in a source selector, or None.

    A source selector lists only enabled sources, so a disabled calendar or
    address book has no cell.
    """
    return find(evolution(), "table cell", name, showing=True)


def select_source(name):
    """Select the calendar or address book `name` in the source selector."""
    cell = wait_for(lambda: source_cell(name), f"{name!r} in the source selector")
    # As in the folder tree: focus moves the cursor, and the cursor selects the
    # row. The delete actions act on the selected source.
    cell.queryComponent().grabFocus()
    say(f"selected source {name!r}")


def info_bar(prefix):
    """Return the showing info bar whose text starts with `prefix`, or None."""
    for bar in descendants(evolution(), "info bar"):
        if is_showing(bar) and find(bar, None, prefix=prefix):
            return bar
    return None


def info_bar_texts(bar):
    """Return the texts of the labels of an info bar, in tree order."""
    return [_name(label) for label in descendants(bar, "label") if _name(label)]


def evolution_log(log="evolution.log"):
    """Return a log of the scenario (EVOLUTION_JMAP_LOG=trace) without colour codes."""
    path = os.path.join(ARTIFACTS, log)
    try:
        with open(path, encoding="utf-8", errors="replace") as log:
            return re.sub(r"\x1b\[[0-9;]*m", "", log.read())
    except FileNotFoundError:
        return ""


def expect_log(pattern, what, log="evolution.log"):
    """Wait until a line of `log` matches the regular expression.

    The registry and the address book and calendar factories write to
    "dbus.log", because D-Bus starts them.
    """
    regex = re.compile(pattern, re.MULTILINE)
    wait_for(lambda: regex.search(evolution_log(log)), f"the log shows {what}", timeout=10)
    say(f"log: {what}")


def log_count(pattern, log="evolution.log"):
    """Return the number of lines of `log` that match the regular expression."""
    return len(re.findall(pattern, evolution_log(log), re.MULTILINE))


def expect_no_errors(log="evolution.log"):
    """Fail if a crate of evolution-jmap logged an error in `log`."""
    errors = re.findall(r"^.* ERROR jmap_.*$", evolution_log(log), re.MULTILINE)
    if errors:
        raise ScenarioFailure("the log shows errors:\n" + "\n".join(errors[:5]))
    say(f"log: no errors in {log}")


def main(setup, run, component="mail", mock_args=()):
    """Dispatch a step from the command line to the scenario.

    The steps are "setup" and "run". The step "config" prints the Evolution
    component to start on the first line, then one argument of jmap-mockd per
    line. ci/gui-scenario/run.sh reads it.
    """
    step = sys.argv[1] if len(sys.argv) > 1 else "run"
    if step == "config":
        print("\n".join([component, *mock_args]))
        return
    try:
        if step == "setup":
            setup()
        else:
            run()
    except ScenarioFailure as failure:
        say(f"FAIL: {failure}")
        sys.exit(1)
    say(f"{step}: PASS")


if __name__ == "__main__" and sys.argv[1:2] == ["menu-item"]:
    _activate_menu_item(sys.argv[2], sys.argv[3])
