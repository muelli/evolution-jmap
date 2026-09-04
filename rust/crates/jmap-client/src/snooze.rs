// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Snoozing mail: `Email.snoozed`, the vendor extension Cyrus implements
//! (draft-ietf-extra-email-snooze expired without an RFC).
//!
//! No server this project has tested exposes it. Cyrus gates the capability
//! behind `jmap_nonstandard_extensions`, and Fastmail — a Cyrus deployment
//! whose *web UI* snoozes — does not enable it on the public JMAP API:
//! probed 2026-09-04, `using` the capability is answered
//! `unknownCapability` (HTTP 400), `Email/get` of `snoozed` is
//! `invalidArguments`, there is no `snoozed`-role mailbox, and
//! `snoozedUntil` is an `unsupportedSort`. Stalwart implements the mailbox
//! role only. So this path is written to the extension as specified and
//! exercised against the mock; it lights up for a Cyrus install with
//! nonstandard extensions on, and correctly stays dark elsewhere.
//!
//! Everything snooze-shaped in this client lives here and in
//! [`jmap_proto::mail::SnoozeDetails`], so the day the draft revives as an
//! RFC the change is a capability constant and whatever fields it renames,
//! not a hunt through the crate. Gate callers on
//! [`jmap_proto::session::Account::has_capability`] with
//! [`CAPABILITY_CYRUS_MAIL`]: a server without it (Stalwart, for one) has no
//! wake-up machinery, and offering snooze there would strand messages in a
//! folder nothing ever empties.

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
    /// the message into `snoozed_mailbox_id` — *only* there, the whole
    /// `mailboxIds` replaced, which is what snoozing means in every client
    /// that has it (the message leaves the inbox until it wakes) and spares
    /// the caller mapping its current folder to a mailbox id. Setting
    /// `snoozed` on a message not entering the snoozed mailbox is refused
    /// server-side.
    ///
    /// The server wakes the message at `details.until`, moving it to
    /// `details.move_to_mailbox_id` (its inbox when absent) — nothing on the
    /// client side has to remember the appointment, which is the point of
    /// gating on server support.
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
    /// account has none — the storage location RFC 9979 §8.1 standardizes
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
