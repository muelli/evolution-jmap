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
//! Two `EExtension`s use one implementation ([`action`]): the mail shell view
//! and the detached message window (`EMailBrowser`). Each merges the same
//! submenu into `/mail-message-popup` of its own UI manager. The submenu is a
//! direct child of the popup and does not use the placeholder of the popup
//! (see [`action`]). Each extension gives the actions its own `EMailReader`
//! for the selection and the folder. The menu has presets and no time picker,
//! for the same reasons as scheduled send ([`crate::send_later::schedule`]).

pub mod action;
pub mod browser_ext;
pub mod shell_ext;
