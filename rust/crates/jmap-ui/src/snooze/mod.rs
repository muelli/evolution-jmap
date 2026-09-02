// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Snooze in the message list: put a message away until a chosen morning, on
//! the server.
//!
//! Strictly the *server-side* feature: `Email.snoozed` as Fastmail deploys it
//! (the Cyrus vendor capability — see [`jmap_proto::mail::SnoozeDetails`] for
//! the whole standardization story). A server without it, Stalwart included,
//! has no wake-up machinery, so there the submenu stays insensitive with a
//! tooltip saying why: a snooze that only this machine remembers would strand
//! messages in a folder nothing empties, and GNOME/evolution#374 is the
//! upstream place for a client-side concept.
//!
//! Two `EExtension`s share one implementation ([`action`]): the mail shell
//! view and the detached message window (`EMailBrowser`), each merging the
//! same submenu into its own GtkUIManager's `/mail-message-popup` — appended
//! directly, since 3.52 offers no third-party placeholder there — and each
//! handing the actions its own `EMailReader` for the selection and folder.
//! Presets rather than a picker, like scheduled send and for the same
//! reasons ([`crate::send_later::schedule`]).

pub mod action;
pub mod browser_ext;
pub mod shell_ext;
