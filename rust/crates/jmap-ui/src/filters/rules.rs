// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The rule model of the Server Filters page's rules editor.
//!
//! A [`Rule`] is Evolution's own model, independent of Sieve. [`crate::filters::script`]
//! turns a list of rules into Sieve source text and back. This module has no FFI
//! and is unit-tested directly.

use std::ffi::CStr;

use jmap_backend_core::i18n::N_;
use serde::{Deserialize, Serialize};

/// One filter rule: a test and the actions it runs when the test matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub name: String,
    pub enabled: bool,
    pub combine: Combine,
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
    pub stop: bool,
}

/// How a rule's conditions combine: all of them, or any one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Combine {
    All,
    Any,
}

/// One condition: a field, how it compares, and the value to compare against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Condition {
    pub field: Field,
    pub op: Op,
    pub value: String,
}

/// The message field a condition tests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Field {
    From,
    To,
    Cc,
    ToOrCc,
    Subject,
    Header(String),
    Size,
}

/// How a condition compares its field to its value.
///
/// `Over` and `Under` compare [`Field::Size`] only; the value is a whole
/// number of kilobytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    Contains,
    NotContains,
    Is,
    Matches,
    Over,
    Under,
}

/// What a rule does when its test matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    MoveTo(Folder),
    CopyTo(Folder),
    MarkRead,
    Flag,
    Redirect { address: String, keep: bool },
    Discard,
}

/// A JMAP mailbox: an id for `:mailboxid`, and a display path for plain `fileinto`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
}

/// The error for a rule with no action.
pub const NO_ACTION: &CStr = N_(c"Add at least one action");
/// The error for a value that holds a newline or a NUL character.
pub const BAD_VALUE: &CStr = N_(c"The value cannot contain a newline or NUL character");
/// The error for a header name outside RFC 5322's allowed characters.
pub const BAD_HEADER_NAME: &CStr =
    N_(c"A header name cannot contain a colon, space, control or non-ASCII character");
/// The error for a size comparison whose value is not a positive integer.
pub const BAD_SIZE_VALUE: &CStr = N_(c"Size must be a whole number of kilobytes greater than zero");
/// The error for 'More than'/'Less than' on a field other than Size, or Size
/// compared with anything else.
pub const SIZE_OP_MISMATCH: &CStr =
    N_(c"'More than' and 'Less than' compare the message size only");
/// The error for a Redirect address without exactly one '@'.
pub const BAD_ADDRESS: &CStr = N_(c"The address must contain exactly one '@'");
/// The error for Discard combined with an action that also delivers the message.
pub const DISCARD_WITH_OTHER: &CStr =
    N_(c"Discard cannot be combined with Move to, Copy to or Redirect");

impl Rule {
    /// Checks the rule against the constraints [`crate::filters::script`] relies on.
    pub fn validate(&self) -> Result<(), &'static CStr> {
        if !no_newline_or_nul(&self.name) {
            return Err(BAD_VALUE);
        }
        if self.actions.is_empty() {
            return Err(NO_ACTION);
        }
        for condition in &self.conditions {
            condition.validate()?;
        }
        for action in &self.actions {
            action.validate()?;
        }
        let discard = self.actions.iter().any(|a| matches!(a, Action::Discard));
        let delivers = self.actions.iter().any(|a| {
            matches!(
                a,
                Action::MoveTo(_) | Action::CopyTo(_) | Action::Redirect { .. }
            )
        });
        if discard && delivers {
            return Err(DISCARD_WITH_OTHER);
        }
        Ok(())
    }
}

impl Condition {
    fn validate(&self) -> Result<(), &'static CStr> {
        if !no_newline_or_nul(&self.value) {
            return Err(BAD_VALUE);
        }
        if let Field::Header(name) = &self.field
            && !is_valid_header_name(name)
        {
            return Err(BAD_HEADER_NAME);
        }
        let is_size = matches!(self.field, Field::Size);
        let is_size_op = matches!(self.op, Op::Over | Op::Under);
        if is_size != is_size_op {
            return Err(SIZE_OP_MISMATCH);
        }
        if is_size && !matches!(self.value.parse::<u64>(), Ok(n) if n > 0) {
            return Err(BAD_SIZE_VALUE);
        }
        Ok(())
    }
}

impl Action {
    fn validate(&self) -> Result<(), &'static CStr> {
        match self {
            Action::MoveTo(folder) | Action::CopyTo(folder) => folder.validate(),
            Action::Redirect { address, .. } => {
                if !no_newline_or_nul(address) {
                    return Err(BAD_VALUE);
                }
                if address.matches('@').count() != 1 {
                    return Err(BAD_ADDRESS);
                }
                Ok(())
            }
            Action::MarkRead | Action::Flag | Action::Discard => Ok(()),
        }
    }
}

impl Folder {
    fn validate(&self) -> Result<(), &'static CStr> {
        if !no_newline_or_nul(&self.id) || !no_newline_or_nul(&self.name) {
            return Err(BAD_VALUE);
        }
        Ok(())
    }
}

/// Whether a value holds neither a newline nor a NUL character.
fn no_newline_or_nul(value: &str) -> bool {
    !value.contains('\n') && !value.contains('\0')
}

/// Whether a name is a valid RFC 5322 §3.6.8 `field-name`: one or more
/// printable US-ASCII characters other than ':' (33-57 and 59-126).
fn is_valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| (33..=57).contains(&b) || (59..=126).contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule_with_actions(actions: Vec<Action>) -> Rule {
        Rule {
            name: "Test".to_owned(),
            enabled: true,
            combine: Combine::All,
            conditions: Vec::new(),
            actions,
            stop: false,
        }
    }

    #[test]
    fn rejects_no_action() {
        assert_eq!(rule_with_actions(Vec::new()).validate(), Err(NO_ACTION));
    }

    #[test]
    fn accepts_a_single_action() {
        assert_eq!(rule_with_actions(vec![Action::MarkRead]).validate(), Ok(()));
    }

    #[test]
    fn rejects_a_newline_in_the_rule_name() {
        let mut rule = rule_with_actions(vec![Action::MarkRead]);
        rule.name = "line one\nline two".to_owned();
        assert_eq!(rule.validate(), Err(BAD_VALUE));
    }

    #[test]
    fn rejects_a_nul_in_a_condition_value() {
        let mut rule = rule_with_actions(vec![Action::MarkRead]);
        rule.conditions.push(Condition {
            field: Field::Subject,
            op: Op::Contains,
            value: "a\0b".to_owned(),
        });
        assert_eq!(rule.validate(), Err(BAD_VALUE));
    }

    #[test]
    fn accepts_a_valid_header_name() {
        let mut rule = rule_with_actions(vec![Action::MarkRead]);
        rule.conditions.push(Condition {
            field: Field::Header("X-Spam-Score".to_owned()),
            op: Op::Contains,
            value: "9".to_owned(),
        });
        assert_eq!(rule.validate(), Ok(()));
    }

    #[test]
    fn rejects_a_header_name_with_a_colon() {
        let mut rule = rule_with_actions(vec![Action::MarkRead]);
        rule.conditions.push(Condition {
            field: Field::Header("X-Spam:Score".to_owned()),
            op: Op::Contains,
            value: "9".to_owned(),
        });
        assert_eq!(rule.validate(), Err(BAD_HEADER_NAME));
    }

    #[test]
    fn rejects_a_header_name_with_a_space() {
        let mut rule = rule_with_actions(vec![Action::MarkRead]);
        rule.conditions.push(Condition {
            field: Field::Header("X Spam Score".to_owned()),
            op: Op::Contains,
            value: "9".to_owned(),
        });
        assert_eq!(rule.validate(), Err(BAD_HEADER_NAME));
    }

    #[test]
    fn rejects_over_on_a_field_other_than_size() {
        let mut rule = rule_with_actions(vec![Action::MarkRead]);
        rule.conditions.push(Condition {
            field: Field::Subject,
            op: Op::Over,
            value: "10".to_owned(),
        });
        assert_eq!(rule.validate(), Err(SIZE_OP_MISMATCH));
    }

    #[test]
    fn rejects_contains_on_the_size_field() {
        let mut rule = rule_with_actions(vec![Action::MarkRead]);
        rule.conditions.push(Condition {
            field: Field::Size,
            op: Op::Contains,
            value: "10".to_owned(),
        });
        assert_eq!(rule.validate(), Err(SIZE_OP_MISMATCH));
    }

    #[test]
    fn accepts_a_positive_size_value() {
        let mut rule = rule_with_actions(vec![Action::MarkRead]);
        rule.conditions.push(Condition {
            field: Field::Size,
            op: Op::Over,
            value: "100".to_owned(),
        });
        assert_eq!(rule.validate(), Ok(()));
    }

    #[test]
    fn rejects_a_zero_size_value() {
        let mut rule = rule_with_actions(vec![Action::MarkRead]);
        rule.conditions.push(Condition {
            field: Field::Size,
            op: Op::Over,
            value: "0".to_owned(),
        });
        assert_eq!(rule.validate(), Err(BAD_SIZE_VALUE));
    }

    #[test]
    fn rejects_a_non_numeric_size_value() {
        let mut rule = rule_with_actions(vec![Action::MarkRead]);
        rule.conditions.push(Condition {
            field: Field::Size,
            op: Op::Under,
            value: "huge".to_owned(),
        });
        assert_eq!(rule.validate(), Err(BAD_SIZE_VALUE));
    }

    #[test]
    fn accepts_a_redirect_address_with_one_at_sign() {
        let rule = rule_with_actions(vec![Action::Redirect {
            address: "person@example.com".to_owned(),
            keep: false,
        }]);
        assert_eq!(rule.validate(), Ok(()));
    }

    #[test]
    fn rejects_a_redirect_address_with_no_at_sign() {
        let rule = rule_with_actions(vec![Action::Redirect {
            address: "person-example.com".to_owned(),
            keep: false,
        }]);
        assert_eq!(rule.validate(), Err(BAD_ADDRESS));
    }

    #[test]
    fn rejects_a_redirect_address_with_two_at_signs() {
        let rule = rule_with_actions(vec![Action::Redirect {
            address: "a@b@example.com".to_owned(),
            keep: false,
        }]);
        assert_eq!(rule.validate(), Err(BAD_ADDRESS));
    }

    #[test]
    fn rejects_discard_combined_with_move_to() {
        let rule = rule_with_actions(vec![
            Action::Discard,
            Action::MoveTo(Folder {
                id: "M1".to_owned(),
                name: "Archive".to_owned(),
            }),
        ]);
        assert_eq!(rule.validate(), Err(DISCARD_WITH_OTHER));
    }

    #[test]
    fn accepts_discard_alone() {
        let rule = rule_with_actions(vec![Action::Discard]);
        assert_eq!(rule.validate(), Ok(()));
    }

    #[test]
    fn accepts_discard_with_mark_read_and_flag() {
        let rule = rule_with_actions(vec![Action::Discard, Action::MarkRead, Action::Flag]);
        assert_eq!(rule.validate(), Ok(()));
    }

    #[test]
    fn rejects_a_folder_name_with_a_newline() {
        let rule = rule_with_actions(vec![Action::CopyTo(Folder {
            id: "M1".to_owned(),
            name: "In\nbox".to_owned(),
        })]);
        assert_eq!(rule.validate(), Err(BAD_VALUE));
    }
}
