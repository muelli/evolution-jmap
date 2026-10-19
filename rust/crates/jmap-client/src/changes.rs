// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Incremental state synchronization via `/changes` (RFC 8620 §5.2).

use std::collections::{BTreeMap, BTreeSet};

use jmap_proto::methods::{ChangesRequest, ChangesResponse};
use jmap_proto::session::{
    CAPABILITY_CALENDARS, CAPABILITY_CONTACTS, CAPABILITY_CORE, CAPABILITY_MAIL,
    CAPABILITY_SUBMISSION,
};
use jmap_proto::{Id, State};

use crate::client::Client;
use crate::error::Error;

impl Client {
    /// Request changes for a data type (`Foo/changes`, RFC 8620 §5.2).
    pub fn changes(
        &self,
        account_id: &Id,
        type_name: &str,
        since_state: &State,
    ) -> Result<ChangesResponse, Error> {
        let capability = match type_name {
            "Mailbox" | "Email" | "Thread" => CAPABILITY_MAIL,
            "AddressBook" | "ContactCard" => CAPABILITY_CONTACTS,
            "Calendar" | "CalendarEvent" | "ParticipantIdentity" | "CalendarEventNotification" => {
                CAPABILITY_CALENDARS
            }
            "EmailSubmission" => CAPABILITY_SUBMISSION,
            other => {
                return Err(Error::Protocol(format!(
                    "no known capability for data type {other}"
                )));
            }
        };
        let request = ChangesRequest {
            account_id: account_id.clone(),
            since_state: since_state.clone(),
            max_changes: None,
        };
        let arguments = self.single_call(
            &[CAPABILITY_CORE, capability],
            &format!("{type_name}/changes"),
            &request,
        )?;
        Ok(serde_json::from_value(arguments)?)
    }
}

/// Maximum number of change pages before terminating pagination.
const MAX_CHANGES_PAGES: usize = 1024;

/// Changes for a data type between two states.
///
/// Identifiers are stored in sets to eliminate duplicates across pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSet {
    /// The state to ask from next time.
    pub new_state: State,
    pub created: BTreeSet<Id>,
    pub updated: BTreeSet<Id>,
    pub destroyed: BTreeSet<Id>,
}

impl ChangeSet {
    /// The answer for a caller that is already up to date.
    fn nothing(new_state: State) -> Self {
        Self {
            new_state,
            created: BTreeSet::new(),
            updated: BTreeSet::new(),
            destroyed: BTreeSet::new(),
        }
    }

    /// Whether nothing changed at all.
    pub fn is_empty(&self) -> bool {
        self.created.is_empty() && self.updated.is_empty() && self.destroyed.is_empty()
    }
}

/// History of an object across a sync window.
#[derive(Default)]
struct Disposition {
    created: bool,
    updated: bool,
    destroyed: bool,
}

impl Client {
    /// Fetch and fold all changes since `since_state` (RFC 8620 §5.2).
    ///
    /// Automatically follows `hasMoreChanges` pages up to `MAX_CHANGES_PAGES`.
    /// Consolidates intermediate states into a canonical [`ChangeSet`].
    pub fn all_changes(
        &self,
        account_id: &Id,
        type_name: &str,
        since_state: &State,
    ) -> Result<ChangeSet, Error> {
        let mut state = since_state.clone();
        let mut by_id: BTreeMap<Id, Disposition> = BTreeMap::new();

        for _ in 0..MAX_CHANGES_PAGES {
            let response = self.changes(account_id, type_name, &state)?;
            for id in response.created {
                by_id.entry(id).or_default().created = true;
            }
            for id in response.updated {
                by_id.entry(id).or_default().updated = true;
            }
            for id in response.destroyed {
                by_id.entry(id).or_default().destroyed = true;
            }

            let advanced = response.new_state != state;
            state = response.new_state;
            if !response.has_more_changes {
                return Ok(ChangeSet::classify(state, by_id));
            }
            // Reject responses that do not advance state.
            if !advanced {
                return Err(Error::Protocol(format!(
                    "{type_name}/changes reports more changes without advancing the state"
                )));
            }
        }
        Err(Error::Protocol(format!(
            "{type_name}/changes never stopped reporting more changes"
        )))
    }
}

impl ChangeSet {
    fn classify(new_state: State, by_id: BTreeMap<Id, Disposition>) -> Self {
        let mut set = Self::nothing(new_state);
        for (id, disposition) in by_id {
            match (disposition.created, disposition.destroyed) {
                // Discard objects created and destroyed in the same window.
                (true, true) => {}
                (true, false) => {
                    set.created.insert(id);
                }
                (false, true) => {
                    set.destroyed.insert(id);
                }
                (false, false) if disposition.updated => {
                    set.updated.insert(id);
                }
                (false, false) => {}
            }
        }
        set
    }
}
