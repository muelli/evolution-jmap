// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The round trips of the Server Filters page, over the [`crate::link`]
//! connection. The calls block, so use them on worker threads only.

use std::collections::BTreeMap;

use jmap_client::sieve::{SieveCapability, SieveScript, SieveScriptSetRequest};
use jmap_client::{expect_created, expect_destroyed, expect_updated};
use jmap_proto::Id;
use jmap_proto::mail::Mailbox;
use jmap_proto::methods::SetRequest;
use serde_json::json;

use crate::filters::rules::Folder;
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
    /// Every mailbox of the account, for the folder combo of the rule
    /// dialog. Empty when the server offers no Sieve: the page shows
    /// nothing to put a folder in.
    pub folders: Vec<Folder>,
}

/// Loads the account's Sieve capability, every script it holds with its
/// text, and the mailboxes a rule can file into (`SieveScript/get`, one
/// blob download per script, and `Mailbox/get`).
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
    let folders = if capability.is_some() {
        folders(link)?
    } else {
        Vec::new()
    };
    Ok(Loaded {
        capability,
        scripts: loaded,
        folders,
    })
}

/// Asks the server whether `text` is a valid Sieve script (RFC 9661
/// Section 2.5), without writing anything. `Ok(None)` means it is valid;
/// `Ok(Some(..))` carries what the server objects to. This is the "Check"
/// button, the one action of the page that does not wait for OK.
pub fn check(link: &AccountLink, text: &str) -> Result<Option<String>, String> {
    let account = link.features.account_id.clone();
    let uploaded = link
        .call(|client| client.upload_blob_slice(&account, "application/sieve", text.as_bytes()))
        .map_err(|error| link::describe(&error))?;
    let validated = link
        .call(|client| client.sieve_script_validate(&account, uploaded.blob_id.clone()))
        .map_err(|error| link::describe(&error))?;
    Ok(validated
        .error
        .map(|error| error.description.unwrap_or(error.error_type)))
}

/// The separator between the names of a mailbox and its parent in the
/// `fileinto` argument of a generated rule.
///
/// Both servers tested deliver with "/" and with no other separator; see
/// decision D7 of the plan this page follows.
const FOLDER_SEPARATOR: &str = "/";

/// How far up a parent chain this module walks before it gives up. A
/// mailbox tree is a few levels deep, so a longer chain means the server
/// answered with a cycle. The page still has to finish.
const MAX_FOLDER_DEPTH: usize = 64;

/// Every mailbox of the account, as the rule dialog's folder combo offers
/// it: the JMAP id for `fileinto :mailboxid`, and the full path for plain
/// `fileinto`. The list is sorted by path.
pub fn folders(link: &AccountLink) -> Result<Vec<Folder>, String> {
    let account = link.features.account_id.clone();
    let answer = link
        .call(|client| client.mailbox_get(&account))
        .map_err(|error| link::describe(&error))?;
    let by_id: BTreeMap<&Id, &Mailbox> = answer
        .list
        .iter()
        .filter_map(|mailbox| mailbox.id.as_ref().map(|id| (id, mailbox)))
        .collect();
    let mut folders: Vec<Folder> = by_id
        .iter()
        .map(|(id, mailbox)| Folder {
            id: id.to_string(),
            name: folder_path(mailbox, &by_id),
        })
        .collect();
    folders.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(folders)
}

/// The path of `mailbox`: its own name, with the name of each parent in
/// front of it.
///
/// A parent the answer does not hold ends the walk, so the mailbox keeps
/// the part of its path that is known. A cycle ends it the same way, after
/// [`MAX_FOLDER_DEPTH`] steps.
fn folder_path(mailbox: &Mailbox, by_id: &BTreeMap<&Id, &Mailbox>) -> String {
    let mut names = vec![mailbox.name.as_str()];
    let mut parent = mailbox.parent_id.as_ref();
    for _ in 0..MAX_FOLDER_DEPTH {
        let Some(found) = parent.and_then(|id| by_id.get(id)) else {
            break;
        };
        names.push(found.name.as_str());
        parent = found.parent_id.as_ref();
    }
    names.reverse();
    names.join(FOLDER_SEPARATOR)
}

/// A script a save creates or updates, before the server assigns it an id.
#[derive(Debug)]
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
#[derive(Debug)]
pub enum ActivationTarget {
    /// The script a `ScriptWrite::Create` of the same plan introduces.
    Created(String),
    /// An already existing script.
    Existing(Id),
}

/// Whether and what a save activates, the third step of D4's submit order.
#[derive(Debug, Default)]
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
#[derive(Debug, Default)]
pub struct SavePlan {
    pub writes: Vec<ScriptWrite>,
    pub destroys: Vec<Id>,
    pub activation: ActivationChange,
    /// True when the save must deactivate the account's active script in a
    /// request of its own, before the writes. RFC 9661 Section 2.4 applies
    /// `onSuccessDeactivateScript` after the creates, updates and destroys,
    /// so a destroy of the active script in the same request still fails
    /// with `sieveIsActive`.
    pub deactivate_first: bool,
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

    if plan.deactivate_first {
        let request = SieveScriptSetRequest::new(SetRequest::<SieveScript>::new(account.clone()))
            .deactivating();
        link.call(|client| client.sieve_script_set(&request))
            .map_err(|error| link::describe(&error))?;
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

    use jmap_proto::Id;

    use super::{
        ActivationChange, ActivationTarget, SavePlan, ScriptWrite, check, folders, load, save,
    };
    use crate::link::AccountLink;
    use crate::session_cache::AccountFeatures;

    #[test]
    fn folders_lists_every_mailbox_by_its_full_path() {
        let server = MockServer::builder().start();
        let (inbox, child, grandchild, top) = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&server.account_id()).unwrap();
            let inbox = account.seed_mailbox("Inbox", Some("inbox"));
            let child = account.seed_child_mailbox("Invoices", None, &inbox);
            let grandchild = account.seed_child_mailbox("2026", None, &child);
            let top = account.seed_mailbox("Archive", None);
            (inbox, child, grandchild, top)
        };

        let folders = folders(&link_to(&server)).unwrap();

        let by_id = |id: &Id| {
            folders
                .iter()
                .find(|folder| folder.id == id.to_string())
                .map(|folder| folder.name.clone())
        };
        assert_eq!(by_id(&inbox).as_deref(), Some("Inbox"));
        assert_eq!(by_id(&child).as_deref(), Some("Inbox/Invoices"));
        assert_eq!(by_id(&grandchild).as_deref(), Some("Inbox/Invoices/2026"));
        assert_eq!(by_id(&top).as_deref(), Some("Archive"));
    }

    #[test]
    fn folders_are_sorted_by_path_so_the_combo_reads_in_order() {
        let server = MockServer::builder().start();
        {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&server.account_id()).unwrap();
            let inbox = account.seed_mailbox("Inbox", Some("inbox"));
            account.seed_mailbox("Archive", None);
            account.seed_child_mailbox("Invoices", None, &inbox);
        }

        let folders = folders(&link_to(&server)).unwrap();

        let names: Vec<&str> = folders.iter().map(|folder| folder.name.as_str()).collect();
        assert_eq!(names, ["Archive", "Inbox", "Inbox/Invoices"]);
    }

    #[test]
    fn a_mailbox_whose_parent_is_not_in_the_answer_keeps_its_own_name() {
        let server = MockServer::builder().start();
        {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&server.account_id()).unwrap();
            account.seed_child_mailbox("Orphan", None, &Id::new("no-such-mailbox"));
        }

        let folders = folders(&link_to(&server)).unwrap();

        let names: Vec<&str> = folders.iter().map(|folder| folder.name.as_str()).collect();
        assert_eq!(names, ["Orphan"]);
    }

    #[test]
    fn a_parent_cycle_does_not_hang_the_page() {
        let server = MockServer::builder().start();
        let (first, second) = {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&server.account_id()).unwrap();
            let first = account.seed_mailbox("One", None);
            let second = account.seed_child_mailbox("Two", None, &first);
            // A server that answers with a cycle is broken, but the page
            // must still finish and show something.
            account
                .mailboxes
                .get_mut(&first)
                .unwrap()
                .parent_id
                .replace(second.clone());
            (first, second)
        };

        let folders = folders(&link_to(&server)).unwrap();

        assert_eq!(folders.len(), 2);
        for id in [first, second] {
            let folder = folders
                .iter()
                .find(|folder| folder.id == id.to_string())
                .expect("every mailbox is offered");
            assert!(!folder.name.is_empty());
        }
    }

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
            deactivate_first: false,
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
            deactivate_first: false,
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
            deactivate_first: false,
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
                deactivate_first: false,
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
                    deactivate_first: false,
                },
            ),
            Ok(())
        );

        assert!(load(&link).unwrap().scripts.is_empty());
    }

    #[test]
    fn save_deactivates_before_it_destroys_the_active_script() {
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
                activation: ActivationChange::Activate(ActivationTarget::Created("new".to_owned())),
                deactivate_first: false,
            },
        )
        .unwrap();
        let id = load(&link).unwrap().scripts[0].script.id.clone().unwrap();

        // Without the separate deactivation the server refuses the destroy:
        // RFC 9661 Section 2.4 runs `onSuccessDeactivateScript` last.
        assert!(
            save(
                &link,
                &SavePlan {
                    writes: Vec::new(),
                    destroys: vec![id.clone()],
                    activation: ActivationChange::Deactivate,
                    deactivate_first: false,
                },
            )
            .is_err(),
            "the active script must not be destroyable in one request"
        );

        assert_eq!(
            save(
                &link,
                &SavePlan {
                    writes: Vec::new(),
                    destroys: vec![id],
                    activation: ActivationChange::Keep,
                    deactivate_first: true,
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
                deactivate_first: false,
            },
        )
        .unwrap_err();

        assert!(error.contains("broken"), "error names the script: {error}");
        assert!(load(&link).unwrap().scripts.is_empty());
    }

    #[test]
    fn check_accepts_a_valid_script_and_writes_nothing() {
        let server = MockServer::builder().start();
        let link = link_to(&server);

        assert_eq!(check(&link, "keep;\n"), Ok(None));

        assert!(load(&link).unwrap().scripts.is_empty());
    }

    #[test]
    fn check_reports_what_the_server_objects_to() {
        let server = MockServer::builder().start();
        let link = link_to(&server);

        let answer = check(&link, "if true {\n").unwrap();

        assert!(answer.is_some(), "the server must refuse an open block");
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
                deactivate_first: false,
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
