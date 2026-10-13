// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Snoozing mail: `Email.snoozed`, the vendor extension Cyrus implements
//! (draft-ietf-extra-email-snooze expired without an RFC).
//!
//! Cyrus gates the capability behind `jmap_nonstandard_extensions`. Stalwart
//! implements the mailbox role only. This client path is written to the
//! extension as specified and exercised against the mock.
//!
//! Everything snooze-shaped in this client lives here and in
//! [`jmap_proto::mail::SnoozeDetails`]. Callers should check
//! [`jmap_proto::session::Account::has_capability`] with
//! [`CAPABILITY_CYRUS_MAIL`]: a server without it has no wake-up machinery,
//! and offering snooze there would strand messages in a folder nothing empties.

use jmap_proto::Id;
use jmap_proto::mail::{Email, Mailbox, SnoozeDetails, role};
use jmap_proto::methods::{SetRequest, SetResponse};
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_CYRUS_MAIL, CAPABILITY_MAIL};

use crate::client::Client;
use crate::contacts::set_failure;
use crate::error::Error;

const USING: &[&str] = &[CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_CYRUS_MAIL];

impl Client {
    /// Snooze one message: an `Email/set` update writing `snoozed` and moving
    /// the message into `snoozed_mailbox_id` (only there, replacing `mailboxIds`),
    /// removing the message from the inbox until it wakes. Setting `snoozed` on a
    /// message not entering the snoozed mailbox is refused server-side.
    ///
    /// The server wakes the message at `details.until`, moving it to
    /// `details.move_to_mailbox_id` (its inbox when absent).
    pub fn snooze_email(
        &self,
        account_id: &Id,
        email_id: &Id,
        snoozed_mailbox_id: &Id,
        details: &SnoozeDetails,
    ) -> Result<(), Error> {
        let mut patch = serde_json::Map::new();
        patch.insert("snoozed".to_owned(), serde_json::to_value(details)?);
        patch.insert(
            "mailboxIds".to_owned(),
            serde_json::json!({ snoozed_mailbox_id.as_str(): true }),
        );
        let request = SetRequest::<Email>::new(account_id.clone())
            .update(email_id.clone(), serde_json::Value::Object(patch));
        let arguments = self.single_call(USING, "Email/set", &request)?;
        let response: SetResponse<Email> = serde_json::from_value(arguments)?;
        if response
            .updated
            .as_ref()
            .is_some_and(|updated| updated.contains_key(email_id))
        {
            return Ok(());
        }
        Err(set_failure(
            response
                .not_updated
                .as_ref()
                .and_then(|map| map.get(email_id)),
        ))
    }

    /// The account's snoozed-role mailbox, created (named "Snoozed") when the
    /// account has none: the storage location RFC 9979 §8.1 standardizes
    /// even though the snoozing mechanism itself is vendor territory.
    pub fn snoozed_mailbox(&self, account_id: &Id) -> Result<Mailbox, Error> {
        if let Some(mailbox) = self
            .mailbox_get(account_id)?
            .list
            .into_iter()
            .find(|mailbox| mailbox.role.as_deref() == Some(role::SNOOZED))
        {
            return Ok(mailbox);
        }
        self.mailbox_create(
            account_id,
            &Mailbox::new("Snoozed").with_role(role::SNOOZED),
        )
    }
}
