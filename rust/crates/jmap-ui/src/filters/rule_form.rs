// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The pure part of the Server Filters page's rules mode: the managed
//! script's rule list, and one rule's own editing dialog.
//!
//! Both models are plain Rust with no FFI, built and read by the widget
//! layer the same way [`crate::filters::form`] already is for the script
//! list. [`RulesEditor`] holds the managed script's rules in Sieve's run
//! order; [`RuleDraft`] holds one rule's fields while a dialog edits them.

use std::ffi::CStr;

use jmap_backend_core::i18n::N_;

use crate::filters::rules::{Action, Combine, Condition, Rule};

pub const NO_RULE_SELECTED: &CStr = N_(c"Select a rule first.");
pub const ALREADY_FIRST: &CStr = N_(c"This rule is already first.");
pub const ALREADY_LAST: &CStr = N_(c"This rule is already last.");
pub const NO_CONDITION_SELECTED: &CStr = N_(c"Select a condition first.");
pub const NO_ACTION_SELECTED: &CStr = N_(c"Select an action first.");

/// The managed script's rules, in the order Sieve runs them, with the row
/// the list view shows selected.
#[derive(Debug, Default)]
pub struct RulesEditor {
    rules: Vec<Rule>,
    selected: Option<usize>,
}

impl RulesEditor {
    /// Builds the editor from the rules `script::load` returned for the
    /// managed script. Selects the first row, if there is one.
    pub fn from_rules(rules: Vec<Rule>) -> Self {
        let selected = (!rules.is_empty()).then_some(0);
        Self { rules, selected }
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Takes the rules back out, in order, for `script::generate_script`.
    pub fn into_rules(self) -> Vec<Rule> {
        self.rules
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Selects a row, or nothing. An index past the end selects nothing.
    pub fn select(&mut self, index: Option<usize>) {
        self.selected = index.filter(|index| *index < self.rules.len());
    }

    /// Appends `rule` from the Add dialog and selects it. Returns its row.
    pub fn add(&mut self, rule: Rule) -> usize {
        self.rules.push(rule);
        let index = self.rules.len() - 1;
        self.selected = Some(index);
        index
    }

    /// Replaces the rule at `index` with what the Edit dialog built.
    pub fn replace(&mut self, index: usize, rule: Rule) -> Result<(), &'static CStr> {
        let slot = self.rules.get_mut(index).ok_or(NO_RULE_SELECTED)?;
        *slot = rule;
        Ok(())
    }

    pub fn remove(&mut self, index: usize) -> Result<(), &'static CStr> {
        if index >= self.rules.len() {
            return Err(NO_RULE_SELECTED);
        }
        self.rules.remove(index);
        self.selected = (!self.rules.is_empty()).then(|| index.min(self.rules.len() - 1));
        Ok(())
    }

    /// Swaps row `index` with the row before it. Sieve runs rules in list
    /// order, so moving a rule changes which one wins when both match.
    pub fn move_up(&mut self, index: usize) -> Result<(), &'static CStr> {
        if index >= self.rules.len() {
            return Err(NO_RULE_SELECTED);
        }
        if index == 0 {
            return Err(ALREADY_FIRST);
        }
        self.rules.swap(index, index - 1);
        self.selected = Some(index - 1);
        Ok(())
    }

    /// Swaps row `index` with the row after it.
    pub fn move_down(&mut self, index: usize) -> Result<(), &'static CStr> {
        if index >= self.rules.len() {
            return Err(NO_RULE_SELECTED);
        }
        if index + 1 >= self.rules.len() {
            return Err(ALREADY_LAST);
        }
        self.rules.swap(index, index + 1);
        self.selected = Some(index + 1);
        Ok(())
    }

    /// Flips the enabled flag of row `index` in place. The page's rule list
    /// owns this flag; [`crate::filters::rule_dialog::edit_rule`] never
    /// changes it.
    pub fn toggle_enabled(&mut self, index: usize) -> Result<(), &'static CStr> {
        let rule = self.rules.get_mut(index).ok_or(NO_RULE_SELECTED)?;
        rule.enabled = !rule.enabled;
        Ok(())
    }

    /// Which buttons the rules list offers for the current selection. Add is
    /// not here: it needs no selection, only rules mode itself.
    pub fn buttons(&self) -> RuleButtons {
        let Some(index) = self.selected else {
            return RuleButtons::default();
        };
        let enabled = self.rules[index].enabled;
        RuleButtons {
            edit: true,
            remove: true,
            up: index > 0,
            down: index + 1 < self.rules.len(),
            enable: !enabled,
            disable: enabled,
        }
    }
}

/// Which buttons of the rules list are usable for the current selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RuleButtons {
    pub edit: bool,
    pub remove: bool,
    pub up: bool,
    pub down: bool,
    pub enable: bool,
    pub disable: bool,
}

/// One rule's own dialog: every field it edits, staged until OK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleDraft {
    pub name: String,
    pub enabled: bool,
    pub combine: Combine,
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
    pub stop: bool,
}

impl RuleDraft {
    /// The draft Add opens with: enabled, no conditions (matches every
    /// message), no actions, stop unset.
    pub fn new() -> Self {
        Self {
            name: String::new(),
            enabled: true,
            combine: Combine::All,
            conditions: Vec::new(),
            actions: Vec::new(),
            stop: false,
        }
    }

    /// The draft Edit opens with: the rule exactly as it stands today.
    pub fn from_rule(rule: &Rule) -> Self {
        Self {
            name: rule.name.clone(),
            enabled: rule.enabled,
            combine: rule.combine,
            conditions: rule.conditions.clone(),
            actions: rule.actions.clone(),
            stop: rule.stop,
        }
    }

    /// Appends a condition row. Returns its index.
    pub fn add_condition(&mut self, condition: Condition) -> usize {
        self.conditions.push(condition);
        self.conditions.len() - 1
    }

    pub fn remove_condition(&mut self, index: usize) -> Result<(), &'static CStr> {
        if index >= self.conditions.len() {
            return Err(NO_CONDITION_SELECTED);
        }
        self.conditions.remove(index);
        Ok(())
    }

    /// Appends an action row. Returns its index.
    pub fn add_action(&mut self, action: Action) -> usize {
        self.actions.push(action);
        self.actions.len() - 1
    }

    pub fn remove_action(&mut self, index: usize) -> Result<(), &'static CStr> {
        if index >= self.actions.len() {
            return Err(NO_ACTION_SELECTED);
        }
        self.actions.remove(index);
        Ok(())
    }

    /// The rule this draft describes, or the first `Rule::validate` error.
    /// The dialog's OK button calls this; a `RulesEditor` never holds an
    /// invalid rule.
    pub fn build(&self) -> Result<Rule, &'static CStr> {
        let rule = Rule {
            name: self.name.clone(),
            enabled: self.enabled,
            combine: self.combine,
            conditions: self.conditions.clone(),
            actions: self.actions.clone(),
            stop: self.stop,
        };
        rule.validate()?;
        Ok(rule)
    }
}

impl Default for RuleDraft {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filters::rules::Field;

    fn rule(name: &str) -> Rule {
        Rule {
            name: name.to_owned(),
            enabled: true,
            combine: Combine::All,
            conditions: Vec::new(),
            actions: vec![Action::MarkRead],
            stop: false,
        }
    }

    #[test]
    fn from_rules_selects_the_first_row() {
        let editor = RulesEditor::from_rules(vec![rule("a"), rule("b")]);
        assert_eq!(editor.selected(), Some(0));
        assert_eq!(editor.rules().len(), 2);
    }

    #[test]
    fn from_rules_of_an_empty_script_selects_nothing() {
        let editor = RulesEditor::from_rules(Vec::new());
        assert_eq!(editor.selected(), None);
    }

    #[test]
    fn add_appends_and_selects_the_new_row() {
        let mut editor = RulesEditor::from_rules(vec![rule("a")]);
        let index = editor.add(rule("b"));
        assert_eq!(index, 1);
        assert_eq!(editor.selected(), Some(1));
        assert_eq!(editor.rules()[1].name, "b");
    }

    #[test]
    fn replace_overwrites_the_row_in_place() {
        let mut editor = RulesEditor::from_rules(vec![rule("a"), rule("b")]);
        editor.replace(0, rule("renamed")).unwrap();
        assert_eq!(editor.rules()[0].name, "renamed");
        assert_eq!(editor.rules()[1].name, "b");
    }

    #[test]
    fn replace_past_the_end_is_refused() {
        let mut editor = RulesEditor::from_rules(vec![rule("a")]);
        assert_eq!(editor.replace(5, rule("x")), Err(NO_RULE_SELECTED));
    }

    #[test]
    fn remove_shifts_selection_to_a_remaining_row() {
        let mut editor = RulesEditor::from_rules(vec![rule("a"), rule("b"), rule("c")]);
        editor.remove(2).unwrap();
        assert_eq!(editor.rules().len(), 2);
        assert_eq!(editor.selected(), Some(1));
    }

    #[test]
    fn remove_of_the_last_row_selects_nothing() {
        let mut editor = RulesEditor::from_rules(vec![rule("a")]);
        editor.remove(0).unwrap();
        assert!(editor.rules().is_empty());
        assert_eq!(editor.selected(), None);
    }

    #[test]
    fn remove_past_the_end_is_refused() {
        let mut editor = RulesEditor::from_rules(vec![rule("a")]);
        assert_eq!(editor.remove(5), Err(NO_RULE_SELECTED));
    }

    #[test]
    fn move_up_swaps_with_the_previous_row() {
        let mut editor = RulesEditor::from_rules(vec![rule("a"), rule("b")]);
        editor.move_up(1).unwrap();
        assert_eq!(editor.rules()[0].name, "b");
        assert_eq!(editor.rules()[1].name, "a");
        assert_eq!(editor.selected(), Some(0));
    }

    #[test]
    fn move_up_the_first_row_is_refused() {
        let mut editor = RulesEditor::from_rules(vec![rule("a"), rule("b")]);
        assert_eq!(editor.move_up(0), Err(ALREADY_FIRST));
    }

    #[test]
    fn move_down_swaps_with_the_next_row() {
        let mut editor = RulesEditor::from_rules(vec![rule("a"), rule("b")]);
        editor.move_down(0).unwrap();
        assert_eq!(editor.rules()[0].name, "b");
        assert_eq!(editor.rules()[1].name, "a");
        assert_eq!(editor.selected(), Some(1));
    }

    #[test]
    fn move_down_the_last_row_is_refused() {
        let mut editor = RulesEditor::from_rules(vec![rule("a"), rule("b")]);
        assert_eq!(editor.move_down(1), Err(ALREADY_LAST));
    }

    #[test]
    fn move_on_an_out_of_range_row_is_refused() {
        let mut editor = RulesEditor::from_rules(vec![rule("a")]);
        assert_eq!(editor.move_up(5), Err(NO_RULE_SELECTED));
        assert_eq!(editor.move_down(5), Err(NO_RULE_SELECTED));
    }

    #[test]
    fn toggle_enabled_flips_the_flag_in_place() {
        let mut editor = RulesEditor::from_rules(vec![rule("a")]);
        assert!(editor.rules()[0].enabled);

        editor.toggle_enabled(0).unwrap();
        assert!(!editor.rules()[0].enabled);

        editor.toggle_enabled(0).unwrap();
        assert!(editor.rules()[0].enabled);
    }

    #[test]
    fn toggle_enabled_past_the_end_is_refused() {
        let mut editor = RulesEditor::from_rules(vec![rule("a")]);
        assert_eq!(editor.toggle_enabled(5), Err(NO_RULE_SELECTED));
    }

    #[test]
    fn buttons_with_no_selection_offers_nothing() {
        let editor = RulesEditor::from_rules(Vec::new());
        assert_eq!(editor.buttons(), RuleButtons::default());
    }

    #[test]
    fn buttons_at_the_only_row_disallows_both_moves() {
        let editor = RulesEditor::from_rules(vec![rule("a")]);
        let buttons = editor.buttons();
        assert!(buttons.edit);
        assert!(buttons.remove);
        assert!(!buttons.up);
        assert!(!buttons.down);
        assert!(buttons.disable, "a fresh rule is enabled");
        assert!(!buttons.enable);
    }

    #[test]
    fn buttons_in_the_middle_allow_both_moves() {
        let mut editor = RulesEditor::from_rules(vec![rule("a"), rule("b"), rule("c")]);
        editor.select(Some(1));
        let buttons = editor.buttons();
        assert!(buttons.up);
        assert!(buttons.down);
    }

    #[test]
    fn buttons_reflect_a_disabled_rule() {
        let mut editor = RulesEditor::from_rules(vec![rule("a")]);
        editor.toggle_enabled(0).unwrap();
        let buttons = editor.buttons();
        assert!(buttons.enable);
        assert!(!buttons.disable);
    }

    #[test]
    fn into_rules_returns_the_rules_in_order() {
        let editor = RulesEditor::from_rules(vec![rule("a"), rule("b")]);
        let rules = editor.into_rules();
        assert_eq!(
            rules.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn a_new_draft_matches_every_message_and_has_no_actions() {
        let draft = RuleDraft::new();
        assert!(draft.enabled);
        assert!(draft.conditions.is_empty());
        assert!(draft.actions.is_empty());
        assert_eq!(draft.combine, Combine::All);
    }

    #[test]
    fn from_rule_copies_every_field() {
        let mut source = rule("Invoices");
        source.conditions.push(Condition {
            field: Field::Subject,
            op: crate::filters::rules::Op::Contains,
            value: "invoice".to_owned(),
        });
        source.stop = true;

        let draft = RuleDraft::from_rule(&source);

        assert_eq!(draft.name, "Invoices");
        assert_eq!(draft.conditions, source.conditions);
        assert_eq!(draft.actions, source.actions);
        assert!(draft.stop);
    }

    #[test]
    fn add_and_remove_condition_rows() {
        let mut draft = RuleDraft::new();
        let index = draft.add_condition(Condition {
            field: Field::Subject,
            op: crate::filters::rules::Op::Contains,
            value: "x".to_owned(),
        });
        assert_eq!(index, 0);
        assert_eq!(draft.conditions.len(), 1);

        draft.remove_condition(0).unwrap();
        assert!(draft.conditions.is_empty());
    }

    #[test]
    fn remove_condition_past_the_end_is_refused() {
        let mut draft = RuleDraft::new();
        assert_eq!(draft.remove_condition(0), Err(NO_CONDITION_SELECTED));
    }

    #[test]
    fn add_and_remove_action_rows() {
        let mut draft = RuleDraft::new();
        let index = draft.add_action(Action::MarkRead);
        assert_eq!(index, 0);

        draft.remove_action(0).unwrap();
        assert!(draft.actions.is_empty());
    }

    #[test]
    fn remove_action_past_the_end_is_refused() {
        let mut draft = RuleDraft::new();
        assert_eq!(draft.remove_action(0), Err(NO_ACTION_SELECTED));
    }

    #[test]
    fn build_succeeds_with_a_name_and_an_action() {
        let mut draft = RuleDraft::new();
        draft.name = "Invoices".to_owned();
        draft.add_action(Action::MarkRead);

        let built = draft.build().unwrap();
        assert_eq!(built.name, "Invoices");
        assert_eq!(built.actions, vec![Action::MarkRead]);
    }

    #[test]
    fn build_without_an_action_fails_validation() {
        let mut draft = RuleDraft::new();
        draft.name = "Empty".to_owned();

        assert_eq!(draft.build(), Err(crate::filters::rules::NO_ACTION));
    }

    #[test]
    fn default_draft_matches_new() {
        assert_eq!(RuleDraft::default(), RuleDraft::new());
    }
}
