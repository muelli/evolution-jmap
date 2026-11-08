#!/bin/bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Entrypoint for the Dovecot oracle image (see ../dovecot-oracle.sh). Creates
# one throwaway IMAP account and runs Dovecot in the foreground on port 143.
#
# DOVECOT_ORACLE_USER names the account. It has a default, so the container
# also works with no environment set at all.
#
# Auth accepts any password, including an empty one, for this user
# (`static`/`nopassword=y`), rather than checking DOVECOT_ORACLE_PASSWORD.
# ci/gui-scenario has no secret service running ("there is no keyring",
# scenario.py's own `dismiss_first_run_dialogs`), so Evolution's mail
# authentication dialog submits an empty password there; this throwaway
# fixture must accept that to be usable from that harness at all.
# DOVECOT_ORACLE_PASSWORD stays only so a caller can still name one, for
# clarity in logs and scripts that reuse this image elsewhere.

set -euo pipefail

USERNAME="${DOVECOT_ORACLE_USER:-tester}"
HOME_DIR="/home/$USERNAME"

if ! id "$USERNAME" >/dev/null 2>&1; then
	useradd -m -d "$HOME_DIR" -s /usr/sbin/nologin "$USERNAME"
fi
mkdir -p "$HOME_DIR/Maildir/cur" "$HOME_DIR/Maildir/new" "$HOME_DIR/Maildir/tmp"
chown -R "$USERNAME:$USERNAME" "$HOME_DIR"

install -d -m 755 /etc/dovecot-oracle

cat >/etc/dovecot-oracle/dovecot.conf <<CONF
protocols = imap
listen = *
ssl = no
disable_plaintext_auth = no
mail_location = maildir:~/Maildir

passdb {
  driver = static
  args = nopassword=y
}
userdb {
  driver = passwd
}

service imap-login {
  inet_listener imap {
    port = 143
  }
}
CONF

exec dovecot -F -c /etc/dovecot-oracle/dovecot.conf
