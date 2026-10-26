// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Mail snoozing extensions (`Email.snoozed`).
//!
//! Implements Cyrus message snoozing extension mechanisms.
//!
//! Callers should verify that the account has [`CAPABILITY_CYRUS_MAIL`].
//! Servers without this capability lack automated message wake-up machinery.

use jmap_proto::Id;
use jmap_proto::mail::{Email, Mailbox, SnoozeDetails, role};
use jmap_proto::methods::{SetRequest, SetResponse};
use jmap_proto::session::{CAPABILITY_CORE, CAPABILITY_CYRUS_MAIL, CAPABILITY_MAIL};

use crate::client::Client;
use crate::error::Error;
use crate::set::expect_updated;

const USING: &[&str] = &[CAPABILITY_CORE, CAPABILITY_MAIL, CAPABILITY_CYRUS_MAIL];

impl Client {
    /// Snooze a message via `Email/set`.
    ///
    /// Sets `snoozed` details and moves the message into `snoozed_mailbox_id`.
    /// The server wakes the message at `details.until` and moves it to `details.move_to_mailbox_id`.
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
        expect_updated(&response, email_id)
    }

    /// Return or create the account's snoozed-role mailbox (RFC 9979 Section 8.1).
    ///
    /// If no mailbox has role `snoozed`, creates a new mailbox named "Snoozed".
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
