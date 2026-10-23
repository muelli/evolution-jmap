// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A mock server with one account, and a `MailSync` over that account.
//!
//! A test file adds its own helpers in its own `impl Fixture` block.

// Each test binary compiles this module separately and uses a subset of it.
#![allow(dead_code)]

use jmap_client::{Client, Credentials};
use jmap_mail_sync::{MailSync, MessageSummary};
use jmap_mock::{AccountState, MockServer, MockServerBuilder};
use jmap_proto::Id;

pub struct Fixture {
    pub server: MockServer,
    pub account_id: Id,
}

impl Fixture {
    pub fn start() -> Self {
        Self::started_with(MockServer::builder())
    }

    /// As [`Fixture::start`], on a server that `builder` configures.
    pub fn started_with(builder: MockServerBuilder) -> Self {
        let server = builder.start();
        let account_id = server.account_id();
        Self { server, account_id }
    }

    pub fn sync(&self) -> MailSync {
        let client = Client::connect(self.server.origin(), Credentials::none()).unwrap();
        MailSync::new(client, self.account_id.clone())
    }

    /// Change the account as a different client does. `Mailbox/changes` and
    /// `Email/changes` report the change.
    pub fn edit<R>(&self, edit: impl FnOnce(&mut AccountState) -> R) -> R {
        let state = self.server.state();
        let mut state = state.lock().unwrap();
        edit(state.account_mut(&self.account_id).unwrap())
    }

    /// Seed a top-level mailbox. `role` is the mailbox role, if any.
    pub fn seed_mailbox(&self, name: &str, role: Option<&str>) -> Id {
        self.edit(|account| account.seed_mailbox(name, role))
    }

    pub fn seed_identity(&self, name: &str, email: &str) -> Id {
        self.edit(|account| account.seed_identity(name, email))
    }

    /// The current contents of the mailbox, as a folder refresh finds them.
    pub fn listing(&self, mailbox: &Id) -> Vec<MessageSummary> {
        let (_, messages) = self.sync().messages(mailbox).expect("the mailbox lists");
        messages
    }
}
