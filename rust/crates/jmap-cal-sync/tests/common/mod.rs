// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A mock server holding two calendars, so that "only this calendar" is
//! something the tests can actually observe.

// Each test binary compiles this module separately and uses a subset of it.
#![allow(dead_code)]

use jmap_cal_sync::CalSync;
use jmap_client::{Client, Credentials};
use jmap_mock::MockServer;
use jmap_proto::Id;
use jmap_proto::calendars::CalendarEvent;
use serde_json::Value;

pub struct Fixture {
    pub server: MockServer,
    pub account_id: Id,
    pub ours: Id,
    pub theirs: Id,
}

impl Fixture {
    pub fn start() -> Self {
        let server = MockServer::builder().start();
        let account_id = server.account_id();
        let (ours, theirs) = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&account_id).unwrap();
            (
                account.seed_calendar("Personal", true),
                account.seed_calendar("Team", false),
            )
        };
        Self {
            server,
            account_id,
            ours,
            theirs,
        }
    }

    pub fn client(&self) -> Client {
        Client::connect(self.server.origin(), Credentials::none()).unwrap()
    }

    /// A [`CalSync`] over the "Personal" calendar.
    pub fn sync(&self) -> CalSync {
        CalSync::new(self.client(), self.account_id.clone(), self.ours.clone())
    }

    /// Create an event directly, bypassing the code under test.
    pub fn seed(&self, calendar: &Id, title: &str, start: &str) -> Id {
        self.client()
            .event_create(
                &self.account_id,
                &CalendarEvent::simple(calendar.clone(), title, start, "PT1H"),
            )
            .unwrap()
            .id
            .expect("server assigned id")
    }

    /// Patch an event directly, bypassing the code under test.
    pub fn patch(&self, id: &Id, patch: Value) {
        self.client()
            .event_update(&self.account_id, id, patch)
            .unwrap();
    }

    pub fn event(&self, id: &Id) -> CalendarEvent {
        self.client()
            .event_get(&self.account_id, std::slice::from_ref(id))
            .unwrap()
            .list
            .into_iter()
            .next()
            .expect("event exists")
    }
}
