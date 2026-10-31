// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The dialog that edits one rule of the managed script.
//!
//! The dialog is modal and stages everything: it returns a validated
//! [`Rule`] when the user clicks OK, and nothing when the user cancels.
//! Nothing here reaches the server.
//!
//! The file has two halves, as [`crate::filters::page`] does. The first is
//! pure: what each combo offers, in which order, and how an index of a
//! combo maps back to the model. It has unit tests. The second half builds
//! the widgets, reads them, and calls the first half. The widgets hold no
//! decision of their own.

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_int};
use std::ptr;

use evo_sys::{
    GTK_ORIENTATION_HORIZONTAL, GTK_ORIENTATION_VERTICAL, GTK_RESPONSE_CANCEL, GTK_RESPONSE_OK,
    GtkWidget, GtkWindow, gtk_box_new, gtk_box_pack_start, gtk_button_new_with_mnemonic,
    gtk_check_button_new_with_mnemonic, gtk_combo_box_get_active, gtk_combo_box_set_active,
    gtk_combo_box_text_append, gtk_combo_box_text_new, gtk_combo_box_text_remove_all,
    gtk_container_add, gtk_container_remove, gtk_dialog_add_button, gtk_dialog_get_content_area,
    gtk_dialog_new, gtk_dialog_run, gtk_entry_new, gtk_label_new, gtk_label_new_with_mnemonic,
    gtk_label_set_mnemonic_widget, gtk_label_set_text, gtk_label_set_xalign, gtk_widget_destroy,
    gtk_widget_get_parent, gtk_widget_get_toplevel, gtk_widget_set_hexpand,
    gtk_widget_set_sensitive, gtk_widget_set_tooltip_text, gtk_widget_set_visible,
    gtk_widget_show_all, gtk_window_get_type, gtk_window_set_title, gtk_window_set_transient_for,
};
use glib_sys::{GFALSE, GTRUE, gpointer};
use gobject_sys::{GObject, GTypeInstance, g_signal_connect_data, g_type_check_instance_is_a};
use jmap_backend_core::callback::gcallback;
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::i18n::{N_, translate, translate_static};
use jmap_backend_core::object_data::ObjectData;
use jmap_backend_core::trampoline::guard;

use crate::filters::rule_form::RuleDraft;
use crate::filters::rules::{Action, Combine, Condition, Field, Folder, Op, Rule};
use crate::widget::{flag, get_bool, get_text, set_bool, set_text};

/// The error when an action that files a message has no folder chosen.
pub const NO_FOLDER_CHOSEN: &CStr = N_(c"Choose a folder for this action");
/// The error when the account holds no folder the rule could file into.
pub const NO_FOLDER_AT_ALL: &CStr = N_(c"This account has no folder to file into");
/// The error when a row of the dialog shows no action yet.
pub const NO_ACTION_CHOSEN: &CStr = N_(c"Choose an action for this row");

/// One item of the field combo, in the order the combo lists them.
///
/// [`Field::Header`] carries the header name, which the combo cannot, so
/// the item here is the plain choice and the name comes from an entry
/// beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldChoice {
    From,
    To,
    Cc,
    ToOrCc,
    Subject,
    Header,
    Size,
}

/// The field combo's items, in order.
pub const FIELD_CHOICES: [FieldChoice; 7] = [
    FieldChoice::From,
    FieldChoice::To,
    FieldChoice::Cc,
    FieldChoice::ToOrCc,
    FieldChoice::Subject,
    FieldChoice::Header,
    FieldChoice::Size,
];

/// The operators a condition on a header offers.
const TEXT_OPS: [Op; 4] = [Op::Contains, Op::NotContains, Op::Is, Op::Matches];
/// The operators a condition on the message size offers.
const SIZE_OPS: [Op; 2] = [Op::Over, Op::Under];

impl FieldChoice {
    pub fn label(self) -> &'static CStr {
        match self {
            Self::From => N_(c"From"),
            Self::To => N_(c"To"),
            Self::Cc => N_(c"Cc"),
            Self::ToOrCc => N_(c"To or Cc"),
            Self::Subject => N_(c"Subject"),
            Self::Header => N_(c"Other header"),
            Self::Size => N_(c"Message size"),
        }
    }

    /// The item the combo shows for `field`, and the header name to put in
    /// the entry beside it (empty for every field but [`Field::Header`]).
    pub fn of(field: &Field) -> (Self, String) {
        match field {
            Field::From => (Self::From, String::new()),
            Field::To => (Self::To, String::new()),
            Field::Cc => (Self::Cc, String::new()),
            Field::ToOrCc => (Self::ToOrCc, String::new()),
            Field::Subject => (Self::Subject, String::new()),
            Field::Header(name) => (Self::Header, name.clone()),
            Field::Size => (Self::Size, String::new()),
        }
    }

    /// The field this item means. `header` is the text of the entry beside
    /// the combo, which only [`FieldChoice::Header`] reads.
    pub fn to_field(self, header: &str) -> Field {
        match self {
            Self::From => Field::From,
            Self::To => Field::To,
            Self::Cc => Field::Cc,
            Self::ToOrCc => Field::ToOrCc,
            Self::Subject => Field::Subject,
            Self::Header => Field::Header(header.to_owned()),
            Self::Size => Field::Size,
        }
    }

    /// True while the entry for the header name belongs beside the combo.
    pub fn needs_header_name(self) -> bool {
        self == Self::Header
    }

    /// The operators the combo beside this field offers, in order.
    ///
    /// The message size compares with "more than" and "less than" only,
    /// and no other field compares with those. `Rule::validate` refuses a
    /// mismatch, which keeps a hand-written rule honest; this list keeps
    /// the dialog from offering one.
    pub fn ops(self) -> &'static [Op] {
        if self == Self::Size {
            &SIZE_OPS
        } else {
            &TEXT_OPS
        }
    }
}

/// The label of one item of an operator combo.
pub fn op_label(op: Op) -> &'static CStr {
    match op {
        Op::Contains => N_(c"contains"),
        Op::NotContains => N_(c"does not contain"),
        Op::Is => N_(c"is"),
        Op::Matches => N_(c"matches"),
        // TRANSLATORS: the value beside this operator is a whole number of
        // kilobytes.
        Op::Over => N_(c"is more than (KB)"),
        Op::Under => N_(c"is less than (KB)"),
    }
}

/// One item of an action combo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionChoice {
    MoveTo,
    CopyTo,
    MarkRead,
    Flag,
    Redirect,
    Discard,
}

/// Every action item, in the order the combo lists the ones it offers.
pub const ACTION_CHOICES: [ActionChoice; 6] = [
    ActionChoice::MoveTo,
    ActionChoice::CopyTo,
    ActionChoice::MarkRead,
    ActionChoice::Flag,
    ActionChoice::Redirect,
    ActionChoice::Discard,
];

impl ActionChoice {
    pub fn label(self) -> &'static CStr {
        match self {
            Self::MoveTo => N_(c"Move to folder"),
            Self::CopyTo => N_(c"Copy to folder"),
            Self::MarkRead => N_(c"Mark as read"),
            Self::Flag => N_(c"Flag the message"),
            Self::Redirect => N_(c"Redirect to address"),
            Self::Discard => N_(c"Discard"),
        }
    }

    /// The item the combo shows for `action`.
    pub fn of(action: &Action) -> Self {
        match action {
            Action::MoveTo(_) => Self::MoveTo,
            Action::CopyTo(_) => Self::CopyTo,
            Action::MarkRead => Self::MarkRead,
            Action::Flag => Self::Flag,
            Action::Redirect { .. } => Self::Redirect,
            Action::Discard => Self::Discard,
        }
    }

    /// The Sieve extensions this action needs the server to support.
    ///
    /// The same set `crate::filters::script` puts in the `require` line.
    /// Redirect and Discard are in the base language and need none.
    pub fn required_extensions(self) -> &'static [&'static str] {
        match self {
            Self::MoveTo => &["fileinto"],
            Self::CopyTo => &["fileinto", "copy"],
            Self::MarkRead | Self::Flag => &["imap4flags"],
            Self::Redirect | Self::Discard => &[],
        }
    }

    /// True while the folder combo of the row belongs to this action.
    pub fn needs_folder(self) -> bool {
        matches!(self, Self::MoveTo | Self::CopyTo)
    }

    /// True while the address entry of the row belongs to this action.
    pub fn needs_address(self) -> bool {
        self == Self::Redirect
    }

    /// True while the "Keep a copy" check box of the row belongs to this
    /// action.
    pub fn offers_keep(self) -> bool {
        self == Self::Redirect
    }
}

/// The action items a server supports, in order.
///
/// Decision D7 of the plan: an action whose extension the server does not
/// list is not offered at all, so the user cannot build a rule the server
/// would refuse.
pub fn actions_for(extensions: &[String]) -> Vec<ActionChoice> {
    ACTION_CHOICES
        .into_iter()
        .filter(|choice| {
            choice
                .required_extensions()
                .iter()
                .all(|needed| has(extensions, needed))
        })
        .collect()
}

/// True while "Keep a copy" belongs beside a Redirect action: the copy is
/// `redirect :copy`, which needs the `copy` extension.
pub fn redirect_keep_offered(extensions: &[String]) -> bool {
    has(extensions, "copy")
}

fn has(extensions: &[String], name: &str) -> bool {
    extensions.iter().any(|extension| extension == name)
}

/// The condition one row of the dialog describes.
pub fn condition_from(choice: FieldChoice, header: &str, op: Op, value: &str) -> Condition {
    Condition {
        field: choice.to_field(header),
        op,
        value: value.to_owned(),
    }
}

/// The action one row of the dialog describes, or why the row is not
/// finished yet.
///
/// `folder` is the mailbox the row's folder combo names, which is `None`
/// when the combo has no selection or the account has no mailbox at all.
pub fn action_from(
    choice: ActionChoice,
    folder: Option<&Folder>,
    address: &str,
    keep: bool,
    any_folder: bool,
) -> Result<Action, &'static CStr> {
    let folder = || match folder {
        Some(folder) => Ok(folder.clone()),
        None if any_folder => Err(NO_FOLDER_CHOSEN),
        None => Err(NO_FOLDER_AT_ALL),
    };
    Ok(match choice {
        ActionChoice::MoveTo => Action::MoveTo(folder()?),
        ActionChoice::CopyTo => Action::CopyTo(folder()?),
        ActionChoice::MarkRead => Action::MarkRead,
        ActionChoice::Flag => Action::Flag,
        ActionChoice::Redirect => Action::Redirect {
            address: address.to_owned(),
            keep,
        },
        ActionChoice::Discard => Action::Discard,
    })
}

/// The row of `folders` a [`Folder`] of an existing action sits in.
///
/// A rule can name a mailbox the account no longer has, so this returns
/// `None` and the row opens with no folder chosen.
pub fn folder_row(folders: &[Folder], folder: &Folder) -> Option<usize> {
    folders.iter().position(|known| known.id == folder.id)
}

/// The item of a combo a [`Combine`] sits in, and back. The combo lists
/// "all" first, as [`Combine::All`] is the default of a new rule.
pub const COMBINE_CHOICES: [Combine; 2] = [Combine::All, Combine::Any];

pub fn combine_label(combine: Combine) -> &'static CStr {
    match combine {
        // TRANSLATORS: this completes "Match ... of the following".
        Combine::All => N_(c"all"),
        Combine::Any => N_(c"any"),
    }
}

/// The item of a list that an index of a combo names.
///
/// `gtk_combo_box_get_active` answers -1 for "nothing chosen", and a
/// refilled combo can answer an index the list no longer has. Both become
/// `None` here, so no caller indexes a slice with a number GTK gave it.
pub fn item_at<T: Copy>(items: &[T], index: c_int) -> Option<T> {
    usize::try_from(index)
        .ok()
        .and_then(|at| items.get(at))
        .copied()
}

// The widget half. Everything above is pure and unit-tested; everything
// below reads a widget, calls the half above, and shows the answer.

/// The qdata of the dialog, for the duration of one [`edit_rule`] call.
const STATE: ObjectData<RefCell<DialogState>> = ObjectData::new(c"jmap-rule-dialog-state");

const RULE_NAME_LABEL: &CStr = N_(c"_Name:");
const MATCH_LABEL: &CStr = N_(c"_Match:");
const CONDITIONS_LABEL: &CStr = N_(c"If the message matches:");
const ACTIONS_LABEL: &CStr = N_(c"Then:");
const ADD_CONDITION: &CStr = N_(c"Add _condition");
const ADD_ACTION: &CStr = N_(c"Add _action");
const REMOVE_ROW: &CStr = N_(c"_Remove");
const STOP_LABEL: &CStr = N_(c"_Stop processing more rules");
const HEADER_NAME_HINT: &CStr = N_(c"Header name");

/// One condition row: the widgets that describe it, and the box that holds
/// them. Removing the row removes `container`, which destroys the rest.
struct ConditionRow {
    container: *mut GtkWidget,
    field: *mut GtkWidget,
    header: *mut GtkWidget,
    op: *mut GtkWidget,
    value: *mut GtkWidget,
    /// True while the operator combo holds the size operators. Only a
    /// change of this needs the combo refilled, so a move between two
    /// header fields keeps the operator the user chose.
    size_ops: Cell<bool>,
}

/// One action row, in the same shape as [`ConditionRow`].
struct ActionRow {
    container: *mut GtkWidget,
    action: *mut GtkWidget,
    folder: *mut GtkWidget,
    address: *mut GtkWidget,
    keep: *mut GtkWidget,
}

/// Everything one open dialog holds. The widgets own the row structure
/// while the dialog is open: a [`RuleDraft`] seeds the rows when the dialog
/// opens, and [`read_draft`] builds a new one when the user clicks OK.
struct DialogState {
    folders: Vec<Folder>,
    /// The action items this server allows, in combo order.
    offered: Vec<ActionChoice>,
    keep_offered: bool,
    /// Carried from the opening draft to the built rule. The rule list of
    /// the page owns this flag, not this dialog.
    enabled: bool,
    name: *mut GtkWidget,
    combine: *mut GtkWidget,
    stop: *mut GtkWidget,
    status: *mut GtkWidget,
    conditions: *mut GtkWidget,
    actions: *mut GtkWidget,
    condition_rows: Vec<ConditionRow>,
    action_rows: Vec<ActionRow>,
}

/// The state of a live dialog.
///
/// # Safety
///
/// `dialog` must be the live dialog of an [`edit_rule`] call. Do not keep
/// the reference past the current callback.
unsafe fn state<'a>(dialog: *mut GObject) -> Option<&'a RefCell<DialogState>> {
    // SAFETY: per the contract. `edit_rule` sets the data one time.
    unsafe { STATE.get(dialog) }
}

/// Edits one rule in a modal dialog. `None` means the user cancelled.
///
/// The dialog stays open while the rule it describes is not valid yet, and
/// shows the reason in its status label. Nothing here reaches the server.
///
/// # Safety
///
/// `page` must be a live widget of the account editor, on the main thread.
/// The function runs a nested main loop, so no caller may hold a borrow of
/// the page state across the call.
pub unsafe fn edit_rule(
    page: *mut GObject,
    title: &'static CStr,
    draft: &RuleDraft,
    folders: &[Folder],
    extensions: &[String],
) -> Option<Rule> {
    // SAFETY: the page is live, per the contract. Every widget below is
    // new and floating, and the container it goes into sinks it. The
    // dialog is the one widget this function destroys.
    unsafe {
        let dialog = gtk_dialog_new();
        gtk_window_set_title(dialog.cast(), translate_static(title));
        parent_on(dialog, page);
        gtk_dialog_add_button(
            dialog.cast(),
            translate_static(N_(c"_Cancel")),
            GTK_RESPONSE_CANCEL,
        );
        gtk_dialog_add_button(dialog.cast(), translate_static(N_(c"_OK")), GTK_RESPONSE_OK);

        let content = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        gtk_container_add(gtk_dialog_get_content_area(dialog.cast()).cast(), content);

        let top = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        gtk_box_pack_start(content.cast(), top, GFALSE, GFALSE, 0);
        let name = labelled_entry(top, RULE_NAME_LABEL, &draft.name);

        let combine = labelled_combo(
            top,
            MATCH_LABEL,
            COMBINE_CHOICES.iter().map(|item| combine_label(*item)),
        );
        set_combo(combine, index_of(&COMBINE_CHOICES, draft.combine));

        section_label(content, CONDITIONS_LABEL);
        let conditions = gtk_box_new(GTK_ORIENTATION_VERTICAL, 3);
        gtk_box_pack_start(content.cast(), conditions, GFALSE, GFALSE, 0);
        add_row_button(dialog, content, ADD_CONDITION, on_add_condition);

        section_label(content, ACTIONS_LABEL);
        let actions = gtk_box_new(GTK_ORIENTATION_VERTICAL, 3);
        gtk_box_pack_start(content.cast(), actions, GFALSE, GFALSE, 0);
        add_row_button(dialog, content, ADD_ACTION, on_add_action);

        let stop = gtk_check_button_new_with_mnemonic(translate_static(STOP_LABEL));
        set_bool(stop.cast(), draft.stop);
        gtk_box_pack_start(content.cast(), stop, GFALSE, GFALSE, 0);

        let status = gtk_label_new(ptr::null());
        gtk_label_set_xalign(status.cast(), 0.0);
        gtk_box_pack_start(content.cast(), status, GFALSE, GFALSE, 0);

        STATE.set(
            dialog.cast(),
            RefCell::new(DialogState {
                folders: folders.to_vec(),
                offered: actions_for(extensions),
                keep_offered: redirect_keep_offered(extensions),
                enabled: draft.enabled,
                name,
                combine,
                stop,
                status,
                conditions,
                actions,
                condition_rows: Vec::new(),
                action_rows: Vec::new(),
            }),
        );

        // The dialog is shown before the rows go in, and each row shows
        // itself. The other order would undo the rows' own work:
        // `gtk_widget_show_all` reaches every descendant, so it would show
        // the header-name entry of each row that just hid it.
        gtk_widget_show_all(dialog);

        // The rows go in after the state exists: each row connects signal
        // handlers that look themselves up in it.
        for condition in &draft.conditions {
            append_condition(dialog.cast(), Some(condition));
        }
        for action in &draft.actions {
            append_action(dialog.cast(), Some(action));
        }

        let built = run_until_valid(dialog);
        gtk_widget_destroy(dialog);
        built
    }
}

/// Runs the dialog until the user cancels or describes a valid rule.
///
/// `gtk_dialog_run` returns once per click of OK, so an incomplete rule
/// shows its reason and the loop runs the dialog again. This is decision
/// D4's rule for the page applied to one dialog: the user never loses what
/// is typed because something else is missing.
///
/// # Safety
///
/// `dialog` must be the live dialog, with its state set.
unsafe fn run_until_valid(dialog: *mut GtkWidget) -> Option<Rule> {
    loop {
        // SAFETY: the dialog is live. This runs a nested main loop, so no
        // borrow of the state is held here.
        let response = unsafe { gtk_dialog_run(dialog.cast()) };
        if response != GTK_RESPONSE_OK {
            return None;
        }
        // SAFETY: the dialog is live and no borrow is held.
        let cell = unsafe { state(dialog.cast()) }?;
        // SAFETY: as above. `read_draft` takes its own borrows and drops
        // them before it returns.
        let built = unsafe { read_draft(cell) }.and_then(|draft| draft.build());
        match built {
            Ok(rule) => return Some(rule),
            // SAFETY: the status label is a live child of the dialog.
            Err(reason) => unsafe { set_status(cell, reason) },
        }
    }
}

/// The rule the widgets describe, or the first reason a row is unfinished.
///
/// # Safety
///
/// `cell` must be the state of the live dialog, on the main thread, with no
/// borrow of it held.
unsafe fn read_draft(cell: &RefCell<DialogState>) -> Result<RuleDraft, &'static CStr> {
    let ui = cell.borrow();
    // SAFETY: every widget here is a live child of the dialog, and the
    // reads below are property and combo reads that emit no signal.
    unsafe {
        let mut draft = RuleDraft::new();
        draft.name = get_text(ui.name.cast());
        draft.enabled = ui.enabled;
        draft.stop = get_bool(ui.stop.cast());
        draft.combine = item_at(&COMBINE_CHOICES, active_of(ui.combine)).unwrap_or(Combine::All);

        for row in &ui.condition_rows {
            let choice =
                item_at(&FIELD_CHOICES, active_of(row.field)).unwrap_or(FieldChoice::Subject);
            let op = item_at(choice.ops(), active_of(row.op)).unwrap_or(choice.ops()[0]);
            draft.add_condition(condition_from(
                choice,
                &get_text(row.header.cast()),
                op,
                &get_text(row.value.cast()),
            ));
        }

        for row in &ui.action_rows {
            let choice = item_at(&ui.offered, active_of(row.action)).ok_or(NO_ACTION_CHOSEN)?;
            let folder = usize::try_from(active_of(row.folder))
                .ok()
                .and_then(|at| ui.folders.get(at));
            draft.add_action(action_from(
                choice,
                folder,
                &get_text(row.address.cast()),
                ui.keep_offered && get_bool(row.keep.cast()),
                !ui.folders.is_empty(),
            )?);
        }
        Ok(draft)
    }
}

/// Appends one condition row, showing `condition` when there is one and a
/// new empty condition otherwise.
///
/// # Safety
///
/// `dialog` must be the live dialog with its state set, and no borrow of
/// the state may be held.
unsafe fn append_condition(dialog: *mut GObject, condition: Option<&Condition>) {
    // SAFETY: the dialog is live, per the contract.
    let Some(cell) = (unsafe { state(dialog) }) else {
        return;
    };
    let (choice, header, op, value) = match condition {
        Some(condition) => {
            let (choice, header) = FieldChoice::of(&condition.field);
            (choice, header, condition.op, condition.value.clone())
        }
        None => (
            FieldChoice::Subject,
            String::new(),
            Op::Contains,
            String::new(),
        ),
    };
    let box_ = cell.borrow().conditions;
    // SAFETY: the box is a live child of the dialog, and no borrow is held
    // across these calls. Every widget is new and its container sinks it.
    let row = unsafe {
        let container = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        gtk_box_pack_start(box_.cast(), container, GFALSE, GFALSE, 0);

        let field = combo_in(container, FIELD_CHOICES.iter().map(|item| item.label()));
        set_combo(field, index_of(&FIELD_CHOICES, choice));

        let header_entry = gtk_entry_new();
        set_text(header_entry.cast(), &header);
        gtk_widget_set_tooltip_text(header_entry, translate_static(HEADER_NAME_HINT));
        gtk_box_pack_start(container.cast(), header_entry, GFALSE, GFALSE, 0);

        let op_combo = combo_in(container, choice.ops().iter().map(|item| op_label(*item)));
        set_combo(op_combo, index_of(choice.ops(), op));

        let value_entry = gtk_entry_new();
        set_text(value_entry.cast(), &value);
        gtk_widget_set_hexpand(value_entry, GTRUE);
        gtk_box_pack_start(container.cast(), value_entry, GTRUE, GTRUE, 0);

        remove_button(dialog, container, on_remove_condition);
        gtk_widget_show_all(container);
        // show_all makes every child visible, so the header entry hides
        // again after it, not before.
        gtk_widget_set_visible(header_entry, flag(choice.needs_header_name()));

        ConditionRow {
            container,
            field,
            header: header_entry,
            op: op_combo,
            value: value_entry,
            size_ops: Cell::new(choice == FieldChoice::Size),
        }
    };
    // The handler finds its row by the widget that emitted, so the row
    // joins the list before the connection, and the connection is last so
    // that nothing above fires it.
    let field = row.field;
    cell.borrow_mut().condition_rows.push(row);
    // SAFETY: the combo is a live child of the dialog, which outlives it.
    unsafe { connect(field, c"changed", on_field_changed, dialog) };
}

/// Appends one action row, as [`append_condition`] does for a condition.
///
/// # Safety
///
/// As [`append_condition`].
unsafe fn append_action(dialog: *mut GObject, action: Option<&Action>) {
    // SAFETY: the dialog is live, per the contract.
    let Some(cell) = (unsafe { state(dialog) }) else {
        return;
    };
    let (box_, offered, folders_empty, keep_offered) = {
        let ui = cell.borrow();
        (
            ui.actions,
            ui.offered.clone(),
            ui.folders.is_empty(),
            ui.keep_offered,
        )
    };
    let choice = action
        .map(ActionChoice::of)
        .filter(|choice| offered.contains(choice))
        .or_else(|| offered.first().copied());
    let chosen_folder = match action {
        Some(Action::MoveTo(folder) | Action::CopyTo(folder)) => {
            let ui = cell.borrow();
            folder_row(&ui.folders, folder)
        }
        _ => None,
    };
    let (address, keep) = match action {
        Some(Action::Redirect { address, keep }) => (address.clone(), *keep),
        _ => (String::new(), false),
    };
    let labels: Vec<&'static CStr> = offered.iter().map(|item| item.label()).collect();
    let folder_labels: Vec<String> = cell
        .borrow()
        .folders
        .iter()
        .map(|folder| folder.name.clone())
        .collect();

    // SAFETY: the box is a live child of the dialog, and no borrow is held
    // across these calls.
    let row = unsafe {
        let container = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        gtk_box_pack_start(box_.cast(), container, GFALSE, GFALSE, 0);

        let action_combo = combo_in(container, labels.into_iter());
        set_combo(
            action_combo,
            choice.and_then(|choice| index_of(&offered, choice)),
        );

        let folder_combo = gtk_combo_box_text_new();
        for label in &folder_labels {
            let label = cstring_lossy(label);
            gtk_combo_box_text_append(folder_combo.cast(), ptr::null(), label.as_ptr());
        }
        set_combo(folder_combo, chosen_folder);
        gtk_box_pack_start(container.cast(), folder_combo, GFALSE, GFALSE, 0);

        let address_entry = gtk_entry_new();
        set_text(address_entry.cast(), &address);
        gtk_widget_set_hexpand(address_entry, GTRUE);
        gtk_box_pack_start(container.cast(), address_entry, GTRUE, GTRUE, 0);

        let keep_check = gtk_check_button_new_with_mnemonic(translate_static(N_(c"_Keep a copy")));
        set_bool(keep_check.cast(), keep);
        gtk_box_pack_start(container.cast(), keep_check, GFALSE, GFALSE, 0);

        remove_button(dialog, container, on_remove_action);
        gtk_widget_show_all(container);

        let row = ActionRow {
            container,
            action: action_combo,
            folder: folder_combo,
            address: address_entry,
            keep: keep_check,
        };
        apply_action_choice(&row, choice, folders_empty, keep_offered);
        row
    };
    let action_combo = row.action;
    cell.borrow_mut().action_rows.push(row);
    // SAFETY: the combo is a live child of the dialog, which outlives it.
    unsafe { connect(action_combo, c"changed", on_action_changed, dialog) };
}

/// Desensitizes the parts of an action row the chosen action does not use.
///
/// # Safety
///
/// Every widget of `row` must be live, on the main thread.
unsafe fn apply_action_choice(
    row: &ActionRow,
    choice: Option<ActionChoice>,
    no_folders: bool,
    keep_offered: bool,
) {
    let needs_folder = choice.is_some_and(ActionChoice::needs_folder);
    // SAFETY: the widgets are live children of the dialog.
    unsafe {
        gtk_widget_set_sensitive(row.folder, flag(needs_folder && !no_folders));
        gtk_widget_set_sensitive(
            row.address,
            flag(choice.is_some_and(ActionChoice::needs_address)),
        );
        // A copy of a redirected message is `redirect :copy`, so a server
        // without the `copy` extension cannot make one.
        gtk_widget_set_sensitive(
            row.keep,
            flag(keep_offered && choice.is_some_and(ActionChoice::offers_keep)),
        );
    }
}

/// The `changed` handler of a field combo. It refills the operator combo of
/// the same row, and shows the header-name entry for "Other header" only.
///
/// # Safety
///
/// The GLib signal system calls this with the dialog as the data pointer.
unsafe extern "C" fn on_field_changed(combo: *mut GObject, dialog: gpointer) {
    guard("JmapRuleDialog::field-changed", (), || {
        // SAFETY: the dialog is live, because its descendant emitted.
        let Some(cell) = (unsafe { state(dialog.cast()) }) else {
            return;
        };
        let ui = cell.borrow();
        let Some(row) = ui
            .condition_rows
            .iter()
            .find(|row| row.field.cast::<GObject>() == combo)
        else {
            return;
        };
        // SAFETY: the widgets are live children of the dialog. Refilling
        // the operator combo emits `changed` on that combo, which has no
        // handler, so this cannot re-enter.
        unsafe {
            let choice =
                item_at(&FIELD_CHOICES, active_of(row.field)).unwrap_or(FieldChoice::Subject);
            gtk_widget_set_visible(row.header, flag(choice.needs_header_name()));
            let size_ops = choice == FieldChoice::Size;
            if size_ops == row.size_ops.get() {
                return;
            }
            row.size_ops.set(size_ops);
            gtk_combo_box_text_remove_all(row.op.cast());
            for op in choice.ops() {
                gtk_combo_box_text_append(
                    row.op.cast(),
                    ptr::null(),
                    translate_static(op_label(*op)),
                );
            }
            set_combo(row.op, Some(0));
        }
    });
}

/// The `changed` handler of an action combo.
///
/// # Safety
///
/// As [`on_field_changed`].
unsafe extern "C" fn on_action_changed(combo: *mut GObject, dialog: gpointer) {
    guard("JmapRuleDialog::action-changed", (), || {
        // SAFETY: the dialog is live, because its descendant emitted.
        let Some(cell) = (unsafe { state(dialog.cast()) }) else {
            return;
        };
        let ui = cell.borrow();
        let Some(row) = ui
            .action_rows
            .iter()
            .find(|row| row.action.cast::<GObject>() == combo)
        else {
            return;
        };
        // SAFETY: the widgets are live children of the dialog, and the
        // sensitivity calls below emit nothing this file handles.
        unsafe {
            let choice = item_at(&ui.offered, active_of(row.action));
            apply_action_choice(row, choice, ui.folders.is_empty(), ui.keep_offered);
        }
    });
}

unsafe extern "C" fn on_add_condition(_button: *mut GObject, dialog: gpointer) {
    guard("JmapRuleDialog::add-condition", (), || {
        // SAFETY: the dialog is live, because its button emitted.
        unsafe { append_condition(dialog.cast(), None) };
    });
}

unsafe extern "C" fn on_add_action(_button: *mut GObject, dialog: gpointer) {
    guard("JmapRuleDialog::add-action", (), || {
        // SAFETY: as above.
        unsafe { append_action(dialog.cast(), None) };
    });
}

unsafe extern "C" fn on_remove_condition(button: *mut GObject, dialog: gpointer) {
    guard("JmapRuleDialog::remove-condition", (), || {
        // SAFETY: as above.
        unsafe { remove_row(dialog.cast(), button, true) };
    });
}

unsafe extern "C" fn on_remove_action(button: *mut GObject, dialog: gpointer) {
    guard("JmapRuleDialog::remove-action", (), || {
        // SAFETY: as above.
        unsafe { remove_row(dialog.cast(), button, false) };
    });
}

/// Takes the row that holds `button` out of the dialog.
///
/// The Remove button of the row is destroyed together with the row it sits
/// in, while its own `clicked` handler runs. GLib holds a reference to the
/// emitting object for the whole emission, so the button outlives this
/// call; nothing here touches it after the removal.
///
/// # Safety
///
/// `dialog` must be the live dialog, and `button` the Remove button of one
/// of its rows. No borrow of the state may be held.
unsafe fn remove_row(dialog: *mut GObject, button: *mut GObject, condition: bool) {
    // SAFETY: the dialog is live, per the contract.
    let Some(cell) = (unsafe { state(dialog) }) else {
        return;
    };
    // The row owns the button, and the button's parent chain reaches the
    // row's own box in one step, which is the container to take out.
    // SAFETY: the button is a live widget.
    let container = unsafe { gtk_widget_get_parent(button.cast()) };
    if container.is_null() {
        return;
    }
    let box_ = {
        let mut ui = cell.borrow_mut();
        if condition {
            ui.condition_rows.retain(|row| row.container != container);
            ui.conditions
        } else {
            ui.action_rows.retain(|row| row.container != container);
            ui.actions
        }
    };
    // SAFETY: the box is a live child of the dialog and `container` is its
    // child. The removal drops the last reference, which destroys the row
    // and its widgets. The state no longer names any of them.
    unsafe { gtk_container_remove(box_.cast(), container) };
}

/// Writes `reason` in the status label of the dialog.
///
/// # Safety
///
/// `cell` must be the state of the live dialog, with no borrow held.
unsafe fn set_status(cell: &RefCell<DialogState>, reason: &'static CStr) {
    let status = cell.borrow().status;
    let text = cstring_lossy(&translate(reason));
    // SAFETY: the label is a live child of the dialog.
    unsafe { gtk_label_set_text(status.cast(), text.as_ptr()) };
}

/// Makes `dialog` transient for the window that holds `page`, when there is
/// one.
///
/// # Safety
///
/// Both must be live widgets, on the main thread.
unsafe fn parent_on(dialog: *mut GtkWidget, page: *mut GObject) {
    // SAFETY: both are live, per the contract. Before the editor packs the
    // page, its toplevel is no window, which GTK allows.
    unsafe {
        let toplevel = gtk_widget_get_toplevel(page.cast());
        if !toplevel.is_null()
            && g_type_check_instance_is_a(toplevel.cast::<GTypeInstance>(), gtk_window_get_type())
                != GFALSE
        {
            gtk_window_set_transient_for(dialog.cast(), toplevel.cast::<GtkWindow>());
        }
    }
}

/// A label and an entry side by side in `box_`. Returns the entry.
///
/// # Safety
///
/// `box_` must be a live `GtkBox`.
unsafe fn labelled_entry(box_: *mut GtkWidget, label: &'static CStr, text: &str) -> *mut GtkWidget {
    // SAFETY: the box is live and both widgets are new.
    unsafe {
        let caption = gtk_label_new_with_mnemonic(translate_static(label));
        let entry = gtk_entry_new();
        set_text(entry.cast(), text);
        gtk_label_set_mnemonic_widget(caption.cast(), entry);
        gtk_box_pack_start(box_.cast(), caption, GFALSE, GFALSE, 0);
        gtk_widget_set_hexpand(entry, GTRUE);
        gtk_box_pack_start(box_.cast(), entry, GTRUE, GTRUE, 0);
        entry
    }
}

/// A label and a filled combo side by side in `box_`. Returns the combo.
///
/// # Safety
///
/// `box_` must be a live `GtkBox`.
unsafe fn labelled_combo(
    box_: *mut GtkWidget,
    label: &'static CStr,
    items: impl Iterator<Item = &'static CStr>,
) -> *mut GtkWidget {
    // SAFETY: the box is live and both widgets are new.
    unsafe {
        let caption = gtk_label_new_with_mnemonic(translate_static(label));
        gtk_box_pack_start(box_.cast(), caption, GFALSE, GFALSE, 0);
        let combo = combo_in(box_, items);
        gtk_label_set_mnemonic_widget(caption.cast(), combo);
        combo
    }
}

/// A combo filled with `items`, packed into `box_`.
///
/// # Safety
///
/// `box_` must be a live `GtkBox`.
unsafe fn combo_in(
    box_: *mut GtkWidget,
    items: impl Iterator<Item = &'static CStr>,
) -> *mut GtkWidget {
    // SAFETY: the box is live and the combo is new.
    unsafe {
        let combo = gtk_combo_box_text_new();
        for item in items {
            gtk_combo_box_text_append(combo.cast(), ptr::null(), translate_static(item));
        }
        gtk_box_pack_start(box_.cast(), combo, GFALSE, GFALSE, 0);
        combo
    }
}

/// A bold-free section caption, left aligned.
///
/// # Safety
///
/// `box_` must be a live `GtkBox`.
unsafe fn section_label(box_: *mut GtkWidget, text: &'static CStr) {
    // SAFETY: the box is live and the label is new.
    unsafe {
        let label = gtk_label_new(translate_static(text));
        gtk_label_set_xalign(label.cast(), 0.0);
        gtk_box_pack_start(box_.cast(), label, GFALSE, GFALSE, 0);
    }
}

/// An "Add ..." button under a section.
///
/// # Safety
///
/// `box_` must be a live `GtkBox` of the live `dialog`.
unsafe fn add_row_button(
    dialog: *mut GtkWidget,
    box_: *mut GtkWidget,
    label: &'static CStr,
    handler: unsafe extern "C" fn(*mut GObject, gpointer),
) {
    // SAFETY: the box is live and the button is new.
    unsafe {
        let button = gtk_button_new_with_mnemonic(translate_static(label));
        gtk_box_pack_start(box_.cast(), button, GFALSE, GFALSE, 0);
        connect(button, c"clicked", handler, dialog.cast());
    }
}

/// The Remove button at the end of one row.
///
/// # Safety
///
/// `container` must be the live row box of the live `dialog`.
unsafe fn remove_button(
    dialog: *mut GObject,
    container: *mut GtkWidget,
    handler: unsafe extern "C" fn(*mut GObject, gpointer),
) {
    // SAFETY: the container is live and the button is new.
    unsafe {
        let button = gtk_button_new_with_mnemonic(translate_static(REMOVE_ROW));
        gtk_box_pack_start(container.cast(), button, GFALSE, GFALSE, 0);
        connect(button, c"clicked", handler, dialog);
    }
}

/// Connects `handler` to `signal` of `widget`, with `dialog` as the data
/// pointer GTK hands back.
///
/// # Safety
///
/// `widget` must be a live descendant of the live `dialog`, so that the
/// data pointer stays valid for as long as the connection does.
unsafe fn connect(
    widget: *mut GtkWidget,
    signal: &'static CStr,
    handler: unsafe extern "C" fn(*mut GObject, gpointer),
    dialog: *mut GObject,
) {
    // SAFETY: per the contract.
    unsafe {
        g_signal_connect_data(
            widget.cast(),
            signal.as_ptr(),
            gcallback(handler),
            dialog.cast(),
            None,
            0,
        );
    }
}

/// The index of the chosen item of a combo, or -1 when none is chosen.
///
/// # Safety
///
/// `combo` must be a live `GtkComboBoxText`, which is a `GtkComboBox`.
unsafe fn active_of(combo: *mut GtkWidget) -> c_int {
    // SAFETY: per the contract. `tests/gtk.rs` in `evo-sys` confirms that
    // GTK still relates the two classes this cast assumes.
    unsafe { gtk_combo_box_get_active(combo.cast()) }
}

/// Chooses item `index` of a combo, or none when `index` is `None`.
///
/// # Safety
///
/// As [`active_of`].
unsafe fn set_combo(combo: *mut GtkWidget, index: Option<usize>) {
    let index = index
        .and_then(|index| c_int::try_from(index).ok())
        .unwrap_or(-1);
    // SAFETY: per the contract. -1 means "choose nothing", which GTK
    // accepts for any combo.
    unsafe { gtk_combo_box_set_active(combo.cast(), index) };
}

/// Where `item` sits in `items`, for a combo that lists them in order.
fn index_of<T: PartialEq>(items: &[T], item: T) -> Option<usize> {
    items.iter().position(|known| *known == item)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extensions(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn every_field_round_trips_through_its_combo_item() {
        let fields = [
            Field::From,
            Field::To,
            Field::Cc,
            Field::ToOrCc,
            Field::Subject,
            Field::Header("X-Spam".to_owned()),
            Field::Size,
        ];
        for field in fields {
            let (choice, header) = FieldChoice::of(&field);
            assert_eq!(choice.to_field(&header), field);
        }
    }

    #[test]
    fn the_field_combo_offers_every_choice_once() {
        assert_eq!(FIELD_CHOICES.len(), 7);
        for choice in FIELD_CHOICES {
            assert_eq!(
                FIELD_CHOICES
                    .iter()
                    .filter(|other| **other == choice)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn only_the_header_choice_asks_for_a_header_name() {
        for choice in FIELD_CHOICES {
            assert_eq!(
                choice.needs_header_name(),
                choice == FieldChoice::Header,
                "{choice:?}"
            );
        }
    }

    #[test]
    fn the_size_field_offers_only_the_size_operators() {
        assert_eq!(FieldChoice::Size.ops(), [Op::Over, Op::Under]);
        for choice in FIELD_CHOICES
            .into_iter()
            .filter(|c| *c != FieldChoice::Size)
        {
            assert_eq!(
                choice.ops(),
                [Op::Contains, Op::NotContains, Op::Is, Op::Matches],
                "{choice:?}"
            );
        }
    }

    #[test]
    fn a_condition_the_combos_describe_passes_validation() {
        use crate::filters::rules::Rule;

        let rule = |condition| Rule {
            name: "Rule".to_owned(),
            enabled: true,
            combine: Combine::All,
            conditions: vec![condition],
            actions: vec![Action::MarkRead],
            stop: false,
        };

        let size = condition_from(FieldChoice::Size, "", Op::Over, "100");
        assert!(rule(size).validate().is_ok());

        let header = condition_from(FieldChoice::Header, "X-Spam", Op::Is, "yes");
        assert_eq!(header.field, Field::Header("X-Spam".to_owned()));
        assert!(rule(header).validate().is_ok());
    }

    #[test]
    fn every_action_round_trips_through_its_combo_item() {
        let folder = Folder {
            id: "M1".to_owned(),
            name: "Invoices".to_owned(),
        };
        let cases = [
            (ActionChoice::MoveTo, Action::MoveTo(folder.clone())),
            (ActionChoice::CopyTo, Action::CopyTo(folder.clone())),
            (ActionChoice::MarkRead, Action::MarkRead),
            (ActionChoice::Flag, Action::Flag),
            (
                ActionChoice::Redirect,
                Action::Redirect {
                    address: "a@example.com".to_owned(),
                    keep: true,
                },
            ),
            (ActionChoice::Discard, Action::Discard),
        ];
        for (choice, action) in cases {
            assert_eq!(ActionChoice::of(&action), choice);
            let built = action_from(choice, Some(&folder), "a@example.com", true, true).unwrap();
            assert_eq!(built, action);
        }
    }

    #[test]
    fn a_server_without_fileinto_offers_neither_move_nor_copy() {
        let offered = actions_for(&extensions(&["imap4flags"]));
        assert!(!offered.contains(&ActionChoice::MoveTo));
        assert!(!offered.contains(&ActionChoice::CopyTo));
        assert!(offered.contains(&ActionChoice::MarkRead));
        assert!(offered.contains(&ActionChoice::Flag));
    }

    #[test]
    fn copy_to_needs_both_fileinto_and_copy() {
        let offered = actions_for(&extensions(&["fileinto"]));
        assert!(offered.contains(&ActionChoice::MoveTo));
        assert!(!offered.contains(&ActionChoice::CopyTo));

        let offered = actions_for(&extensions(&["fileinto", "copy"]));
        assert!(offered.contains(&ActionChoice::CopyTo));
    }

    #[test]
    fn a_server_without_imap4flags_offers_no_flag_action() {
        let offered = actions_for(&extensions(&["fileinto"]));
        assert!(!offered.contains(&ActionChoice::MarkRead));
        assert!(!offered.contains(&ActionChoice::Flag));
    }

    #[test]
    fn redirect_and_discard_need_no_extension() {
        let offered = actions_for(&[]);
        assert_eq!(offered, vec![ActionChoice::Redirect, ActionChoice::Discard]);
    }

    #[test]
    fn keeping_a_copy_of_a_redirect_needs_the_copy_extension() {
        assert!(!redirect_keep_offered(&extensions(&["fileinto"])));
        assert!(redirect_keep_offered(&extensions(&["fileinto", "copy"])));
    }

    #[test]
    fn an_action_that_files_without_a_chosen_folder_is_refused() {
        assert_eq!(
            action_from(ActionChoice::MoveTo, None, "", false, true),
            Err(NO_FOLDER_CHOSEN)
        );
    }

    #[test]
    fn an_account_with_no_folder_says_so_instead() {
        assert_eq!(
            action_from(ActionChoice::CopyTo, None, "", false, false),
            Err(NO_FOLDER_AT_ALL)
        );
    }

    #[test]
    fn an_action_that_needs_no_folder_builds_without_one() {
        assert_eq!(
            action_from(ActionChoice::Discard, None, "", false, false),
            Ok(Action::Discard)
        );
    }

    #[test]
    fn only_the_two_filing_actions_use_the_folder_combo() {
        for choice in ACTION_CHOICES {
            assert_eq!(
                choice.needs_folder(),
                matches!(choice, ActionChoice::MoveTo | ActionChoice::CopyTo),
                "{choice:?}"
            );
            assert_eq!(
                choice.needs_address(),
                choice == ActionChoice::Redirect,
                "{choice:?}"
            );
            assert_eq!(
                choice.offers_keep(),
                choice == ActionChoice::Redirect,
                "{choice:?}"
            );
        }
    }

    #[test]
    fn a_folder_the_account_still_has_finds_its_row() {
        let folders = vec![
            Folder {
                id: "M1".to_owned(),
                name: "Archive".to_owned(),
            },
            Folder {
                id: "M2".to_owned(),
                name: "Invoices".to_owned(),
            },
        ];
        assert_eq!(folder_row(&folders, &folders[1]), Some(1));
    }

    #[test]
    fn a_folder_the_account_lost_finds_no_row() {
        let folders = vec![Folder {
            id: "M1".to_owned(),
            name: "Archive".to_owned(),
        }];
        let gone = Folder {
            id: "M9".to_owned(),
            name: "Gone".to_owned(),
        };
        assert_eq!(folder_row(&folders, &gone), None);
    }

    #[test]
    fn an_empty_combo_answers_minus_one_and_names_no_item() {
        assert_eq!(item_at(&FIELD_CHOICES, -1), None);
        assert_eq!(item_at(&FIELD_CHOICES, 0), Some(FieldChoice::From));
        assert_eq!(item_at(&FIELD_CHOICES, 6), Some(FieldChoice::Size));
    }

    #[test]
    fn an_index_past_the_end_of_a_refilled_combo_names_no_item() {
        // The operator combo is refilled when the field changes. A stale
        // index from the longer list must not index the shorter one.
        assert_eq!(item_at(FieldChoice::Size.ops(), 3), None);
    }

    #[test]
    fn the_combine_combo_lists_all_before_any() {
        assert_eq!(COMBINE_CHOICES, [Combine::All, Combine::Any]);
    }
}
