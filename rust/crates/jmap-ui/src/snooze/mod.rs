// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Snooze in the message list: the server hides a message until a selected
//! time.
//!
//! This is only the *server-side* feature: `Email.snoozed`, the Cyrus vendor
//! capability (see [`jmap_proto::mail::SnoozeDetails`] for the full
//! standardization history).
//!
//! **No server tested to this date has the capability.** Thus, the submenu
//! is insensitive on all servers except the mock. Stalwart has the mailbox
//! role only. The public JMAP of Fastmail refuses the capability, but its web
//! UI can snooze (`jmap_client::snooze` records the probe). The code follows
//! the extension, so it is ready for a Cyrus server with nonstandard
//! extensions enabled.
//!
//! A snooze that only this computer keeps is not a good alternative: it can
//! leave messages in a folder that nothing empties. GNOME/evolution#374 is the
//! upstream issue for a client-side snooze.
//!
//! One `EExtension` on the `EMailReader` interface ([`reader_ext`]) covers
//! both reader windows: the mail window and the detached message window
//! (`EMailBrowser`). It merges the submenu into `/mail-message-popup` (the
//! message list) and `/mail-preview-popup` (the message body) of the UI
//! manager of each window. The submenu is a direct child of each popup and
//! does not use the placeholder of either popup (see [`action`]). The reader
//! of the window gives the selection and the folder. The menu has presets and
//! no time picker, for the same reasons as scheduled send
//! ([`crate::send_later::schedule`]).

pub mod action;
pub mod reader_ext;
