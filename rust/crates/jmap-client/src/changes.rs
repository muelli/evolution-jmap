// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Incremental sync via `/changes` (RFC 8620 §5.2) — the primitive the
//! Evolution Data Server meta-backends' `get_changes_sync` will drive.

use std::collections::{BTreeMap, BTreeSet};

use jmap_proto::methods::{ChangesRequest, ChangesResponse};
use jmap_proto::session::{
    CAPABILITY_CALENDARS, CAPABILITY_CONTACTS, CAPABILITY_CORE, CAPABILITY_MAIL,
};
use jmap_proto::{Id, State};

use crate::client::Client;
use crate::error::Error;

impl Client {
    /// Generic `Foo/changes` call. `type_name` is a JMAP data type such as
    /// `Email` or `ContactCard`.
    pub fn changes(
        &self,
        account_id: &Id,
        type_name: &str,
        since_state: &State,
    ) -> Result<ChangesResponse, Error> {
        let capability = match type_name {
            "Mailbox" | "Email" | "Thread" => CAPABILITY_MAIL,
            "AddressBook" | "ContactCard" => CAPABILITY_CONTACTS,
            "Calendar" | "CalendarEvent" => CAPABILITY_CALENDARS,
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

/// A server that answers `/changes` forever without ever clearing
/// `hasMoreChanges` would otherwise hang the calling thread. The cap is far
/// above any real backlog; reaching it means the server is broken.
const MAX_CHANGES_PAGES: usize = 1024;

/// Everything that changed to one data type between two states.
///
/// The identifiers are sets, not the response's lists: what a caller has to
/// know is *whether* an object changed, and the same object can be named by
/// several pages of one answer.
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

/// One object's history across the window, before it is decided what to call
/// it.
#[derive(Default)]
struct Disposition {
    created: bool,
    updated: bool,
    destroyed: bool,
}

impl Client {
    /// Every change since `since_state`, however many pages the server needs.
    ///
    /// RFC 8620 §5.2 lets a server truncate a `/changes` answer whenever it
    /// likes — `maxChanges` is the client's cap, not the only reason for one —
    /// so a caller that reads a single response is a caller that silently
    /// misses changes. Following `hasMoreChanges` here is what lets the layers
    /// above treat "what changed" as one question.
    ///
    /// The pages are then folded back into the answer a single response would
    /// have carried, by the same rule RFC 8620 §5.2 states for one: an object
    /// created and destroyed inside the window is reported neither way,
    /// because the caller never learned it existed, and one created and then
    /// modified is created. Where the server put the page boundary is not
    /// something a caller should be able to see.
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
            // A page that ends where it started is a page the next call
            // fetches again: the server is asking to be looped over forever.
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
                // Never visible to this client: it is gone, and the client was
                // never told it was there.
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
