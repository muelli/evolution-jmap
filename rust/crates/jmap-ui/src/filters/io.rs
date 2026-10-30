// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The round trips of the Server Filters page, over the [`crate::link`]
//! connection. The calls block, so use them on worker threads only.

use jmap_client::sieve::{SieveCapability, SieveScript, SieveScriptSetRequest};
use jmap_client::{expect_created, expect_destroyed, expect_updated};
use jmap_proto::Id;
use jmap_proto::methods::SetRequest;
use serde_json::json;

use crate::link::{self, AccountLink};

/// One script the server holds, with its Sieve source already downloaded.
pub struct LoadedScript {
    pub script: SieveScript,
    pub text: String,
}

/// Everything the page needs to show after a load.
pub struct Loaded {
    /// `None` when the server does not offer Sieve to this account. The page
    /// shows "Your server does not offer server-side filters." instead.
    pub capability: Option<SieveCapability>,
    pub scripts: Vec<LoadedScript>,
}

/// Loads the account's Sieve capability and every script it holds, with text
/// (`SieveScript/get` plus one blob download per script).
pub fn load(link: &AccountLink) -> Result<Loaded, String> {
    let account = link.features.account_id.clone();
    let capability = link.client.sieve_capability(&account);
    let scripts = link
        .call(|client| client.sieve_scripts(&account))
        .map_err(|error| link::describe(&error))?;
    let mut loaded = Vec::with_capacity(scripts.len());
    for script in scripts {
        let text = link
            .call(|client| client.sieve_script_text(&account, &script))
            .map_err(|error| link::describe(&error))?;
        loaded.push(LoadedScript { script, text });
    }
    Ok(Loaded {
        capability,
        scripts: loaded,
    })
}

/// A script a save creates or updates, before the server assigns it an id.
pub enum ScriptWrite {
    /// A new script, keyed by a client-chosen creation key.
    Create {
        key: String,
        name: String,
        text: String,
    },
    /// A patch to an existing script's name and content.
    Update { id: Id, name: String, text: String },
}

/// Which `ScriptWrite` of a [`SavePlan`] a save activates, if any.
pub enum ActivationTarget {
    /// The script a `ScriptWrite::Create` of the same plan introduces.
    Created(String),
    /// An already existing script.
    Existing(Id),
}

/// Whether and what a save activates, the third step of D4's submit order.
#[derive(Default)]
pub enum ActivationChange {
    /// Leave the account's active script as it is.
    #[default]
    Keep,
    /// Activate the named script once the writes below succeed.
    Activate(ActivationTarget),
    /// Deactivate the account's active script, leaving none active.
    Deactivate,
}

/// Everything one submit of the Server Filters page writes, in the order
/// `notes/SIEVE-FILTERS-PLAN-2026-10-30.md` decision D4 lays out: validate
/// every changed script first and write nothing on the first invalid one,
/// then one `SieveScript/set` with every create, update and destroy, with
/// the activation change in the same request.
#[derive(Default)]
pub struct SavePlan {
    pub writes: Vec<ScriptWrite>,
    pub destroys: Vec<Id>,
    pub activation: ActivationChange,
}

/// Writes `plan` to the server. On an invalid script, returns the server's
/// description prefixed with that script's name, and writes nothing (RFC
/// 9661 Section 2.2 separates an upload from the script it becomes part of;
/// an unused blob left on the server is not a script change).
pub fn save(link: &AccountLink, plan: &SavePlan) -> Result<(), String> {
    let account = link.features.account_id.clone();

    let mut blob_ids = Vec::with_capacity(plan.writes.len());
    for write in &plan.writes {
        let (name, text) = match write {
            ScriptWrite::Create { name, text, .. } => (name, text),
            ScriptWrite::Update { name, text, .. } => (name, text),
        };
        let uploaded = link
            .call(|client| client.upload_blob_slice(&account, "application/sieve", text.as_bytes()))
            .map_err(|error| link::describe(&error))?;
        let validated = link
            .call(|client| client.sieve_script_validate(&account, uploaded.blob_id.clone()))
            .map_err(|error| link::describe(&error))?;
        if let Some(error) = validated.error {
            let detail = error
                .description
                .clone()
                .unwrap_or_else(|| error.error_type.clone());
            return Err(format!("{name}: {detail}"));
        }
        blob_ids.push(uploaded.blob_id);
    }

    let mut set = SetRequest::<SieveScript>::new(account.clone());
    for (write, blob_id) in plan.writes.iter().zip(&blob_ids) {
        match write {
            ScriptWrite::Create { key, name, .. } => {
                set = set.create(key.clone(), SieveScript::new(name.clone(), blob_id.clone()));
            }
            ScriptWrite::Update { id, name, .. } => {
                set = set.update(id.clone(), json!({"name": name, "blobId": blob_id}));
            }
        }
    }
    for id in &plan.destroys {
        set = set.destroy(id.clone());
    }
    let mut request = SieveScriptSetRequest::new(set);
    request = match &plan.activation {
        ActivationChange::Keep => request,
        ActivationChange::Activate(ActivationTarget::Created(key)) => {
            request.activating(format!("#{key}"))
        }
        ActivationChange::Activate(ActivationTarget::Existing(id)) => {
            request.activating(id.clone())
        }
        ActivationChange::Deactivate => request.deactivating(),
    };

    let response = link
        .call(|client| client.sieve_script_set(&request))
        .map_err(|error| link::describe(&error))?;

    for write in &plan.writes {
        match write {
            ScriptWrite::Create { key, .. } => {
                expect_created(&response, key).map_err(|error| link::describe(&error))?;
            }
            ScriptWrite::Update { id, .. } => {
                expect_updated(&response, id).map_err(|error| link::describe(&error))?;
            }
        }
    }
    for id in &plan.destroys {
        expect_destroyed(&response, id).map_err(|error| link::describe(&error))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use jmap_client::{Client, Credentials};
    use jmap_mock::MockServer;

    use super::{ActivationChange, ActivationTarget, SavePlan, ScriptWrite, load, save};
    use crate::link::AccountLink;
    use crate::session_cache::AccountFeatures;

    fn link_to(server: &MockServer) -> AccountLink {
        let client = Client::connect(server.origin(), Credentials::none()).unwrap();
        let features = AccountFeatures::from_session(client.session()).unwrap();
        AccountLink::without_refresh(client, features)
    }

    #[test]
    fn load_returns_the_capability_and_an_empty_script_list() {
        let server = MockServer::builder().start();

        let loaded = load(&link_to(&server)).unwrap();

        assert!(loaded.capability.is_some());
        assert!(loaded.scripts.is_empty());
    }

    #[test]
    fn load_reports_no_capability_for_an_account_without_sieve() {
        let server = MockServer::builder().no_sieve().start();

        let loaded = load(&link_to(&server)).unwrap();

        assert!(loaded.capability.is_none());
    }

    #[test]
    fn save_creates_and_activates_a_script_in_one_submit() {
        let server = MockServer::builder().start();
        let link = link_to(&server);

        let plan = SavePlan {
            writes: vec![ScriptWrite::Create {
                key: "new".to_owned(),
                name: "evolution-jmap".to_owned(),
                text: "keep;\n".to_owned(),
            }],
            destroys: Vec::new(),
            activation: ActivationChange::Activate(ActivationTarget::Created("new".to_owned())),
        };
        assert_eq!(save(&link, &plan), Ok(()));

        let loaded = load(&link).unwrap();
        assert_eq!(loaded.scripts.len(), 1);
        assert_eq!(loaded.scripts[0].script.name, "evolution-jmap");
        assert!(loaded.scripts[0].script.is_active);
        assert_eq!(loaded.scripts[0].text, "keep;\n");
    }

    #[test]
    fn save_renames_and_rewrites_an_existing_script() {
        let server = MockServer::builder().start();
        let link = link_to(&server);
        let plan = SavePlan {
            writes: vec![ScriptWrite::Create {
                key: "new".to_owned(),
                name: "first".to_owned(),
                text: "keep;\n".to_owned(),
            }],
            destroys: Vec::new(),
            activation: ActivationChange::Keep,
        };
        save(&link, &plan).unwrap();
        let id = load(&link).unwrap().scripts[0].script.id.clone().unwrap();

        let update = SavePlan {
            writes: vec![ScriptWrite::Update {
                id: id.clone(),
                name: "renamed".to_owned(),
                text: "discard;\n".to_owned(),
            }],
            destroys: Vec::new(),
            activation: ActivationChange::Keep,
        };
        assert_eq!(save(&link, &update), Ok(()));

        let loaded = load(&link).unwrap();
        assert_eq!(loaded.scripts.len(), 1);
        assert_eq!(loaded.scripts[0].script.id, Some(id));
        assert_eq!(loaded.scripts[0].script.name, "renamed");
        assert_eq!(loaded.scripts[0].text, "discard;\n");
    }

    #[test]
    fn save_destroys_a_script() {
        let server = MockServer::builder().start();
        let link = link_to(&server);
        save(
            &link,
            &SavePlan {
                writes: vec![ScriptWrite::Create {
                    key: "new".to_owned(),
                    name: "first".to_owned(),
                    text: "keep;\n".to_owned(),
                }],
                destroys: Vec::new(),
                activation: ActivationChange::Keep,
            },
        )
        .unwrap();
        let id = load(&link).unwrap().scripts[0].script.id.clone().unwrap();

        assert_eq!(
            save(
                &link,
                &SavePlan {
                    writes: Vec::new(),
                    destroys: vec![id],
                    activation: ActivationChange::Keep,
                },
            ),
            Ok(())
        );

        assert!(load(&link).unwrap().scripts.is_empty());
    }

    #[test]
    fn save_rejects_an_invalid_script_and_writes_nothing() {
        let server = MockServer::builder().start();
        let link = link_to(&server);

        let error = save(
            &link,
            &SavePlan {
                writes: vec![ScriptWrite::Create {
                    key: "new".to_owned(),
                    name: "broken".to_owned(),
                    text: "if true {\n".to_owned(),
                }],
                destroys: Vec::new(),
                activation: ActivationChange::Keep,
            },
        )
        .unwrap_err();

        assert!(error.contains("broken"), "error names the script: {error}");
        assert!(load(&link).unwrap().scripts.is_empty());
    }

    #[test]
    fn save_leaves_the_vacation_script_alone_when_the_plan_does_not_mention_it() {
        let server = MockServer::builder().sieve_vacation_script().start();
        let link = link_to(&server);
        link.call(|client| {
            client.vacation_response_update(
                &link.features.account_id,
                serde_json::json!({"isEnabled": true}),
            )
        })
        .unwrap();
        // Enabling vacation above talks to `VacationResponse/set`, not this
        // module; read the script it created through `SieveScript/get` here.
        let before = load(&link).unwrap();
        assert_eq!(before.scripts.len(), 1);
        assert_eq!(before.scripts[0].script.name, "vacation");

        save(
            &link,
            &SavePlan {
                writes: vec![ScriptWrite::Create {
                    key: "new".to_owned(),
                    name: "evolution-jmap".to_owned(),
                    text: "keep;\n".to_owned(),
                }],
                destroys: Vec::new(),
                activation: ActivationChange::Keep,
            },
        )
        .unwrap();

        let after = load(&link).unwrap();
        assert_eq!(after.scripts.len(), 2);
        let vacation = after
            .scripts
            .iter()
            .find(|loaded| loaded.script.name == "vacation")
            .unwrap();
        assert!(vacation.script.is_active, "vacation stays active");
    }
}
