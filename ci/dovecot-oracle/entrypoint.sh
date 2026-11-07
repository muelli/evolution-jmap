#!/bin/bash
# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Entrypoint for the Dovecot oracle image (see ../dovecot-oracle.sh). Creates
# one throwaway IMAP account and runs Dovecot in the foreground on port 143.
#
# DOVECOT_ORACLE_USER and DOVECOT_ORACLE_PASSWORD name the account. Both have
# defaults, so the container also works with no environment set at all.

set -euo pipefail

USERNAME="${DOVECOT_ORACLE_USER:-tester}"
PASSWORD="${DOVECOT_ORACLE_PASSWORD:-password}"
HOME_DIR="/home/$USERNAME"

if ! id "$USERNAME" >/dev/null 2>&1; then
	useradd -m -d "$HOME_DIR" -s /usr/sbin/nologin "$USERNAME"
fi
mkdir -p "$HOME_DIR/Maildir/cur" "$HOME_DIR/Maildir/new" "$HOME_DIR/Maildir/tmp"
chown -R "$USERNAME:$USERNAME" "$HOME_DIR"

install -d -m 755 /etc/dovecot-oracle
printf '%s:{PLAIN}%s\n' "$USERNAME" "$PASSWORD" >/etc/dovecot-oracle/passwd
chmod 644 /etc/dovecot-oracle/passwd

cat >/etc/dovecot-oracle/dovecot.conf <<CONF
protocols = imap
listen = *
ssl = no
disable_plaintext_auth = no
mail_location = maildir:~/Maildir

passdb {
  driver = passwd-file
  args = /etc/dovecot-oracle/passwd
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
