// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The pure part of the Server Filters page: what the user staged, and what
//! one submit writes.
//!
//! The page stages every change and writes on OK, as the vacation page does.
//! This module holds that staged state and every decision about it: which
//! names a server accepts, which buttons are usable, which script the
//! vacation responder owns, and the [`SavePlan`] the submit sends. The
//! widgets only read and display the answers.

use std::ffi::CStr;

use jmap_backend_core::i18n::{N_, translate_with};
use jmap_proto::Id;
use jmap_proto::sieve::SieveCapability;

use crate::filters::io::{ActivationChange, ActivationTarget, Loaded, SavePlan, ScriptWrite};

/// The names under which a server keeps the script of its own vacation
/// responder: `vacation` on Stalwart, the capability URN on Cyrus. A script
/// with one of these names belongs to the Vacation Responder page, so this
/// page never renames, rewrites, removes or deactivates it.
pub const VACATION_SCRIPT_NAMES: [&str; 2] = ["vacation", "urn:ietf:params:jmap:vacationresponse"];

/// True if the vacation responder owns the script with this name.
pub fn is_vacation_script(name: &str) -> bool {
    VACATION_SCRIPT_NAMES.contains(&name)
}

pub const NAME_EMPTY: &CStr = N_(c"Give the script a name");
pub const NO_SCRIPT_SELECTED: &CStr = N_(c"Select a script first.");
pub const NAME_BAD_CHARS: &CStr = N_(c"A script name cannot contain a newline or NUL character");
pub const NAME_RESERVED: &CStr = N_(c"This name belongs to the vacation responder");
pub const NAME_TAKEN: &CStr = N_(c"Another script already has this name");
pub const NAME_TOO_LONG: &CStr = N_(c"This name is too long for this server");
pub const TOO_MANY_SCRIPTS: &CStr = N_(c"This server holds no more scripts. Remove one first.");
pub const SCRIPT_PROTECTED: &CStr =
    N_(c"The vacation responder owns this script. Change it on the Vacation Responder page.");
pub const VACATION_ACTIVE: &CStr =
    N_(c"Turn off the vacation responder to activate server filters.");
// TRANSLATORS: %1$s is the name of the filter script.
pub const SCRIPT_TOO_LARGE: &CStr = N_(c"%1$s: the script is too large for this server");

/// The Sieve a new script starts with. `keep;` is core Sieve (RFC 5228
/// Section 4.1) and keeps the message, which is what the server does anyway.
pub const NEW_SCRIPT_TEXT: &str = "keep;\n";

/// One script as the page shows it: the server's copy, plus the edits the
/// user staged on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptEntry {
    /// The server id. `None` for a script this session added.
    pub id: Option<Id>,
    /// The creation key a `SieveScript/set` refers to a new script by.
    pub key: String,
    original_name: String,
    original_text: String,
    pub name: String,
    pub text: String,
    /// The activation state the user wants, not the state the server holds.
    pub active: bool,
    /// True for the script the vacation responder owns.
    pub protected: bool,
}

impl ScriptEntry {
    /// True if a submit must write this script.
    pub fn changed(&self) -> bool {
        self.id.is_none() || self.name != self.original_name || self.text != self.original_text
    }
}

/// Which buttons of the page are usable for the current selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Buttons {
    pub add: bool,
    pub rename: bool,
    pub remove: bool,
    pub activate: bool,
    pub deactivate: bool,
    pub check: bool,
    /// True when the text view shows a script. It is then readable, even
    /// for a script this page does not change.
    pub view: bool,
    /// True when the text view accepts edits.
    pub edit: bool,
}

/// Every script of the account, with the changes the user staged.
#[derive(Debug, Default)]
pub struct FiltersForm {
    entries: Vec<ScriptEntry>,
    selected: Option<usize>,
    destroys: Vec<Id>,
    /// The script the server has active, before this session's changes.
    original_active: Option<Id>,
    offered: bool,
    max_size_script: Option<u64>,
    max_number_scripts: Option<u64>,
    max_size_script_name: Option<u64>,
    next_key: u32,
}

impl FiltersForm {
    /// Builds the staged state from one [`crate::filters::io::load`].
    pub fn from_loaded(loaded: Loaded) -> Self {
        let mut form = Self {
            offered: loaded.capability.is_some(),
            ..Self::default()
        };
        if let Some(capability) = &loaded.capability {
            form.max_size_script = Some(capability.max_size_script);
            form.max_number_scripts = capability.max_number_scripts;
            form.max_size_script_name = max_size_script_name(capability);
        }
        for script in loaded.scripts {
            if script.script.is_active {
                form.original_active = script.script.id.clone();
            }
            form.entries.push(ScriptEntry {
                id: script.script.id,
                key: String::new(),
                original_name: script.script.name.clone(),
                original_text: script.text.clone(),
                protected: is_vacation_script(&script.script.name),
                name: script.script.name,
                text: script.text,
                active: script.script.is_active,
            });
        }
        form.selected = (!form.entries.is_empty()).then_some(0);
        form
    }

    /// True when the server offers Sieve to this account.
    pub fn offered(&self) -> bool {
        self.offered
    }

    pub fn entries(&self) -> &[ScriptEntry] {
        &self.entries
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Selects a row, or nothing. An index past the end selects nothing.
    pub fn select(&mut self, index: Option<usize>) {
        self.selected = index.filter(|index| *index < self.entries.len());
    }

    fn entry(&self, index: usize) -> Option<&ScriptEntry> {
        self.entries.get(index)
    }

    /// True while the vacation responder holds the one active script. The
    /// servers allow one active script only, so activating another here
    /// would turn the responder off.
    pub fn vacation_blocks_activation(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.protected && entry.active)
    }

    /// Which buttons the current selection allows.
    pub fn buttons(&self) -> Buttons {
        if !self.offered {
            return Buttons::default();
        }
        let room = self
            .max_number_scripts
            .is_none_or(|limit| (self.entries.len() as u64) < limit);
        let Some(entry) = self.selected.and_then(|index| self.entry(index)) else {
            return Buttons {
                add: room,
                ..Buttons::default()
            };
        };
        let editable = !entry.protected;
        Buttons {
            add: room,
            rename: editable,
            remove: editable,
            activate: editable && !entry.active && !self.vacation_blocks_activation(),
            deactivate: editable && entry.active,
            check: true,
            view: true,
            edit: editable,
        }
    }

    /// Adds a script with `name` and selects it. Returns its row.
    pub fn add(&mut self, name: &str) -> Result<usize, &'static CStr> {
        if self
            .max_number_scripts
            .is_some_and(|limit| (self.entries.len() as u64) >= limit)
        {
            return Err(TOO_MANY_SCRIPTS);
        }
        self.check_name(name, None)?;
        self.next_key += 1;
        self.entries.push(ScriptEntry {
            id: None,
            key: format!("new{}", self.next_key),
            original_name: String::new(),
            original_text: String::new(),
            name: name.to_owned(),
            text: NEW_SCRIPT_TEXT.to_owned(),
            active: false,
            protected: false,
        });
        let index = self.entries.len() - 1;
        self.selected = Some(index);
        Ok(index)
    }

    /// Renames the script in row `index`.
    pub fn rename(&mut self, index: usize, name: &str) -> Result<(), &'static CStr> {
        self.writable(index)?;
        self.check_name(name, Some(index))?;
        self.entries[index].name = name.to_owned();
        Ok(())
    }

    /// Replaces the Sieve source of row `index` with what the text view holds.
    pub fn set_text(&mut self, index: usize, text: &str) -> Result<(), &'static CStr> {
        self.writable(index)?;
        self.entries[index].text = text.to_owned();
        Ok(())
    }

    /// Removes row `index`. A script the server already holds becomes a
    /// destroy in the next [`FiltersForm::plan`].
    pub fn remove(&mut self, index: usize) -> Result<(), &'static CStr> {
        self.writable(index)?;
        let entry = self.entries.remove(index);
        if let Some(id) = entry.id {
            self.destroys.push(id);
        }
        self.selected = (!self.entries.is_empty()).then(|| index.min(self.entries.len() - 1));
        Ok(())
    }

    /// Makes row `index` the script the account runs.
    pub fn activate(&mut self, index: usize) -> Result<(), &'static CStr> {
        self.writable(index)?;
        if self.vacation_blocks_activation() {
            return Err(VACATION_ACTIVE);
        }
        for (row, entry) in self.entries.iter_mut().enumerate() {
            entry.active = row == index;
        }
        Ok(())
    }

    /// Leaves the account with no active script.
    pub fn deactivate(&mut self, index: usize) -> Result<(), &'static CStr> {
        self.writable(index)?;
        for entry in &mut self.entries {
            entry.active = false;
        }
        Ok(())
    }

    /// True when a submit has anything to write.
    pub fn dirty(&self) -> bool {
        !self.destroys.is_empty()
            || self.entries.iter().any(ScriptEntry::changed)
            || self.desired_active() != self.original_active_row()
    }

    /// What one submit writes. `None` when nothing changed: Evolution
    /// submits every page on every save, and a page without changes must
    /// not write.
    pub fn plan(&self) -> Result<Option<SavePlan>, String> {
        if !self.offered || !self.dirty() {
            return Ok(None);
        }
        let mut writes = Vec::new();
        for entry in &self.entries {
            if entry.protected || !entry.changed() {
                continue;
            }
            if self
                .max_size_script
                .is_some_and(|limit| entry.text.len() as u64 > limit)
            {
                return Err(translate_with(SCRIPT_TOO_LARGE, &[&entry.name]));
            }
            writes.push(match &entry.id {
                Some(id) => ScriptWrite::Update {
                    id: id.clone(),
                    name: entry.name.clone(),
                    text: entry.text.clone(),
                },
                None => ScriptWrite::Create {
                    key: entry.key.clone(),
                    name: entry.name.clone(),
                    text: entry.text.clone(),
                },
            });
        }

        // A destroy of the account's active script needs its own earlier
        // deactivation; see `SavePlan::deactivate_first`.
        let deactivate_first = self
            .original_active
            .as_ref()
            .is_some_and(|id| self.destroys.contains(id));
        let activation = match self.desired_active() {
            None if deactivate_first => ActivationChange::Keep,
            None if self.original_active.is_some() => ActivationChange::Deactivate,
            None => ActivationChange::Keep,
            Some(row) => {
                let entry = &self.entries[row];
                match &entry.id {
                    Some(id) if Some(id) == self.original_active.as_ref() => ActivationChange::Keep,
                    Some(id) => ActivationChange::Activate(ActivationTarget::Existing(id.clone())),
                    None => {
                        ActivationChange::Activate(ActivationTarget::Created(entry.key.clone()))
                    }
                }
            }
        };

        Ok(Some(SavePlan {
            writes,
            destroys: self.destroys.clone(),
            activation,
            deactivate_first,
        }))
    }

    /// The row the user wants active.
    fn desired_active(&self) -> Option<usize> {
        self.entries.iter().position(|entry| entry.active)
    }

    /// The row the server has active, if that script is still in the list.
    fn original_active_row(&self) -> Option<usize> {
        let id = self.original_active.as_ref()?;
        self.entries
            .iter()
            .position(|entry| entry.id.as_ref() == Some(id))
    }

    /// Refuses a change to a row that does not exist, or to the script the
    /// vacation responder owns.
    fn writable(&self, index: usize) -> Result<(), &'static CStr> {
        match self.entry(index) {
            None => Err(NO_SCRIPT_SELECTED),
            Some(entry) if entry.protected => Err(SCRIPT_PROTECTED),
            Some(_) => Ok(()),
        }
    }

    /// The name rules both probed servers enforce, checked before the
    /// request rather than after its refusal. The server check stays
    /// authoritative.
    fn check_name(&self, name: &str, skip: Option<usize>) -> Result<(), &'static CStr> {
        if name.trim().is_empty() {
            return Err(NAME_EMPTY);
        }
        if name.contains('\n') || name.contains('\0') {
            return Err(NAME_BAD_CHARS);
        }
        if is_vacation_script(name) {
            return Err(NAME_RESERVED);
        }
        if self
            .max_size_script_name
            .is_some_and(|limit| name.len() as u64 > limit)
        {
            return Err(NAME_TOO_LONG);
        }
        let taken = self
            .entries
            .iter()
            .enumerate()
            .any(|(row, entry)| Some(row) != skip && entry.name == name);
        if taken {
            return Err(NAME_TAKEN);
        }
        Ok(())
    }
}

/// `maxSizeScriptName` of RFC 9661 Section 1.2. `SieveCapability` does not
/// name it, so read it from the properties it kept.
fn max_size_script_name(capability: &SieveCapability) -> Option<u64> {
    capability.extra.get("maxSizeScriptName")?.as_u64()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use jmap_proto::sieve::SieveScript;

    use super::*;
    use crate::filters::io::LoadedScript;

    fn script(id: &str, name: &str, active: bool) -> LoadedScript {
        LoadedScript {
            script: SieveScript::new(name, Id::from("blob"))
                .with_id(Id::from(id))
                .is_active(active),
            text: format!("# {name}\nkeep;\n"),
        }
    }

    fn loaded(scripts: Vec<LoadedScript>) -> Loaded {
        Loaded {
            capability: Some(SieveCapability::new(1024).with_max_number_scripts(3)),
            scripts,
        }
    }

    fn form(scripts: Vec<LoadedScript>) -> FiltersForm {
        FiltersForm::from_loaded(loaded(scripts))
    }

    #[test]
    fn a_fresh_load_is_not_dirty_and_plans_nothing() {
        let form = form(vec![script("a", "work", true), script("b", "spam", false)]);

        assert!(!form.dirty());
        assert!(form.plan().unwrap().is_none());
        assert_eq!(form.selected(), Some(0));
    }

    #[test]
    fn an_account_without_sieve_offers_no_buttons() {
        let form = FiltersForm::from_loaded(Loaded {
            capability: None,
            scripts: Vec::new(),
        });

        assert!(!form.offered());
        assert_eq!(form.buttons(), Buttons::default());
        assert!(form.plan().unwrap().is_none());
    }

    #[test]
    fn add_creates_a_script_and_plans_one_create() {
        let mut form = form(Vec::new());

        let row = form.add("work").unwrap();

        assert_eq!(row, 0);
        assert_eq!(form.selected(), Some(0));
        assert_eq!(form.entries()[0].text, NEW_SCRIPT_TEXT);
        let plan = form.plan().unwrap().unwrap();
        assert_eq!(plan.writes.len(), 1);
        match &plan.writes[0] {
            ScriptWrite::Create { key, name, text } => {
                assert_eq!(name, "work");
                assert_eq!(text, NEW_SCRIPT_TEXT);
                assert!(!key.is_empty());
            }
            ScriptWrite::Update { .. } => panic!("a new script must be a create"),
        }
        assert!(plan.destroys.is_empty());
        assert!(!plan.deactivate_first);
    }

    #[test]
    fn add_refuses_an_empty_a_duplicate_and_a_reserved_name() {
        let mut form = form(vec![script("a", "work", false)]);

        assert_eq!(form.add("  "), Err(NAME_EMPTY));
        assert_eq!(form.add("work"), Err(NAME_TAKEN));
        assert_eq!(form.add("vacation"), Err(NAME_RESERVED));
        assert_eq!(
            form.add("urn:ietf:params:jmap:vacationresponse"),
            Err(NAME_RESERVED)
        );
        assert_eq!(form.add("two\nlines"), Err(NAME_BAD_CHARS));
        assert_eq!(form.entries().len(), 1);
    }

    #[test]
    fn add_stops_at_the_servers_script_count() {
        let mut form = form(vec![script("a", "one", false), script("b", "two", false)]);

        assert!(form.add("three").is_ok());
        assert_eq!(form.add("four"), Err(TOO_MANY_SCRIPTS));
        assert!(!form.buttons().add);
    }

    #[test]
    fn a_name_longer_than_the_servers_limit_is_refused() {
        let mut form = FiltersForm::from_loaded(Loaded {
            capability: Some(SieveCapability::new(1024).with_extra(BTreeMap::from([(
                "maxSizeScriptName".to_owned(),
                serde_json::json!(8),
            )]))),
            scripts: Vec::new(),
        });

        assert_eq!(form.add("123456789"), Err(NAME_TOO_LONG));
        assert!(form.add("12345678").is_ok());
    }

    #[test]
    fn rename_and_edit_plan_one_update() {
        let mut form = form(vec![script("a", "work", false)]);

        form.rename(0, "office").unwrap();
        form.set_text(0, "discard;\n").unwrap();

        let plan = form.plan().unwrap().unwrap();
        assert_eq!(plan.writes.len(), 1);
        match &plan.writes[0] {
            ScriptWrite::Update { id, name, text } => {
                assert_eq!(id, &Id::from("a"));
                assert_eq!(name, "office");
                assert_eq!(text, "discard;\n");
            }
            ScriptWrite::Create { .. } => panic!("an existing script must be an update"),
        }
    }

    #[test]
    fn renaming_back_to_the_loaded_name_writes_nothing() {
        let mut form = form(vec![script("a", "work", false)]);

        form.rename(0, "office").unwrap();
        form.rename(0, "work").unwrap();

        assert!(!form.dirty());
        assert!(form.plan().unwrap().is_none());
    }

    #[test]
    fn remove_of_an_added_script_leaves_no_destroy() {
        let mut form = form(Vec::new());
        form.add("work").unwrap();

        form.remove(0).unwrap();

        assert!(!form.dirty());
        assert!(form.plan().unwrap().is_none());
    }

    #[test]
    fn remove_of_the_active_script_deactivates_first() {
        let mut form = form(vec![script("a", "work", true), script("b", "spam", false)]);

        form.remove(0).unwrap();

        let plan = form.plan().unwrap().unwrap();
        assert_eq!(plan.destroys, vec![Id::from("a")]);
        assert!(plan.deactivate_first);
        assert!(matches!(plan.activation, ActivationChange::Keep));
        assert_eq!(form.selected(), Some(0));
    }

    #[test]
    fn deactivate_plans_a_deactivation() {
        let mut form = form(vec![script("a", "work", true)]);

        form.deactivate(0).unwrap();

        let plan = form.plan().unwrap().unwrap();
        assert!(plan.writes.is_empty());
        assert!(!plan.deactivate_first);
        assert!(matches!(plan.activation, ActivationChange::Deactivate));
    }

    #[test]
    fn activate_plans_the_existing_script_it_names() {
        let mut form = form(vec![script("a", "work", true), script("b", "spam", false)]);

        form.activate(1).unwrap();

        let plan = form.plan().unwrap().unwrap();
        match plan.activation {
            ActivationChange::Activate(ActivationTarget::Existing(id)) => {
                assert_eq!(id, Id::from("b"));
            }
            _ => panic!("the plan must activate the selected script"),
        }
    }

    #[test]
    fn activate_of_an_added_script_names_its_creation_key() {
        let mut form = form(Vec::new());
        let row = form.add("work").unwrap();

        form.activate(row).unwrap();

        let plan = form.plan().unwrap().unwrap();
        let key = match &plan.writes[0] {
            ScriptWrite::Create { key, .. } => key.clone(),
            ScriptWrite::Update { .. } => panic!("a new script must be a create"),
        };
        match plan.activation {
            ActivationChange::Activate(ActivationTarget::Created(named)) => {
                assert_eq!(named, key);
            }
            _ => panic!("the plan must activate the new script"),
        }
    }

    #[test]
    fn activating_the_script_the_server_already_runs_writes_no_activation() {
        let mut form = form(vec![script("a", "work", true), script("b", "spam", false)]);

        form.activate(1).unwrap();
        form.activate(0).unwrap();

        assert!(!form.dirty());
        assert!(form.plan().unwrap().is_none());
    }

    #[test]
    fn the_vacation_script_takes_no_edit_and_no_removal() {
        let mut form = form(vec![
            script("v", "vacation", false),
            script("a", "work", false),
        ]);

        assert!(form.entries()[0].protected);
        form.select(Some(0));
        let buttons = form.buttons();
        assert!(!buttons.rename);
        assert!(!buttons.remove);
        assert!(!buttons.activate);
        assert!(!buttons.deactivate);
        assert!(!buttons.edit);
        assert!(buttons.view, "the script stays readable");
        assert!(buttons.check, "reading the script back is not an edit");
        assert_eq!(form.rename(0, "other"), Err(SCRIPT_PROTECTED));
        assert_eq!(form.remove(0), Err(SCRIPT_PROTECTED));
        assert_eq!(form.set_text(0, "discard;\n"), Err(SCRIPT_PROTECTED));
    }

    #[test]
    fn an_active_vacation_script_blocks_activation_of_another() {
        let mut form = form(vec![
            script("v", "vacation", true),
            script("a", "work", false),
        ]);

        assert!(form.vacation_blocks_activation());
        form.select(Some(1));
        assert!(!form.buttons().activate);
        assert_eq!(form.activate(1), Err(VACATION_ACTIVE));
    }

    #[test]
    fn a_script_over_the_servers_size_limit_names_itself_in_the_error() {
        let mut form = FiltersForm::from_loaded(Loaded {
            capability: Some(SieveCapability::new(16)),
            scripts: vec![script("a", "work", false)],
        });

        form.set_text(0, &"#".repeat(17)).unwrap();

        let error = form.plan().unwrap_err();
        assert!(
            error.contains("work"),
            "the error names the script: {error}"
        );
    }

    #[test]
    fn an_unchanged_vacation_script_keeps_the_plan_empty() {
        let form = form(vec![script("v", "vacation", true)]);

        assert!(!form.dirty());
        assert!(form.plan().unwrap().is_none());
    }
}
