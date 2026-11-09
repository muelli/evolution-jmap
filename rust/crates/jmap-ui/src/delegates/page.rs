// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapDelegatesPage`: the Delegates page of the account editor. It is a
//! `GtkScrolledWindow` subclass that implements `EMailConfigPage`, as
//! [`crate::vacation::page`] is, and it follows that page in every
//! structural decision. Read that file first: it explains the parent
//! choice, the qdata state box and the asynchronous load.
//!
//! This first slice (ROADMAP item 164) lists the delegates
//! [`crate::delegation::account_delegates`] already finds in the account's
//! own calendars and address books, one row each, with the calendar and
//! address book level named in its own column, a Revoke button that clears
//! both levels for the selected row, and an Add button that opens a dialog
//! asking for a principal id and the two levels to grant it. `submit` still
//! always succeeds without a server call: Revoke and Add write at once, on
//! their own click, the same way the Server Filters page's "Check" button
//! does. The principal id is manual entry only; the `Principal/query`
//! picker (`crate::delegation::search_principals`) stays a later claim.
//!
//! The list is a `GtkTreeView` over a `GtkListStore` of three text columns,
//! the same construction as [`crate::filters::page`]: rows go in through
//! `gtk_list_store_insert_with_valuesv`, so this file never allocates a
//! `GtkTreeIter`. The selection is read back as a `GtkTreePath`, the same
//! way [`crate::filters::page`] reads its own list's selection; the
//! principal id of the selected row comes from a plain `Vec` kept parallel
//! to the store's rows, not from the store itself.

use std::cell::RefCell;
use std::ffi::{CStr, c_int};
use std::ptr;
use std::sync::Arc;

use eds_sys::ESource;
use evo_sys::{
    EMailConfigPage, EMailConfigPageInterface, GTK_ORIENTATION_HORIZONTAL,
    GTK_ORIENTATION_VERTICAL, GTK_RESPONSE_CANCEL, GTK_RESPONSE_OK, GTK_SELECTION_BROWSE,
    GtkCellRenderer, GtkListStore, GtkTreeModel, GtkTreePath, GtkTreeSelection, GtkWidget,
    GtkWindow, e_mail_config_page_get_type, gtk_box_new, gtk_box_pack_start,
    gtk_button_new_with_mnemonic, gtk_cell_renderer_text_new, gtk_combo_box_get_active,
    gtk_combo_box_set_active, gtk_combo_box_text_append, gtk_combo_box_text_new, gtk_container_add,
    gtk_container_set_border_width, gtk_dialog_add_button, gtk_dialog_get_content_area,
    gtk_dialog_new, gtk_dialog_run, gtk_dialog_set_default_response, gtk_entry_new,
    gtk_entry_set_activates_default, gtk_label_new, gtk_label_new_with_mnemonic,
    gtk_label_set_mnemonic_widget, gtk_label_set_text, gtk_label_set_xalign, gtk_list_store_clear,
    gtk_list_store_insert_with_valuesv, gtk_list_store_newv, gtk_scrolled_window_get_type,
    gtk_tree_path_free, gtk_tree_path_get_indices_with_depth, gtk_tree_selection_get_selected_rows,
    gtk_tree_selection_set_mode, gtk_tree_view_append_column, gtk_tree_view_column_add_attribute,
    gtk_tree_view_column_new, gtk_tree_view_column_pack_start, gtk_tree_view_column_set_title,
    gtk_tree_view_get_selection, gtk_tree_view_new_with_model, gtk_tree_view_set_headers_visible,
    gtk_widget_destroy, gtk_widget_get_toplevel, gtk_widget_set_hexpand, gtk_widget_set_sensitive,
    gtk_widget_set_vexpand, gtk_widget_show_all, gtk_window_get_type, gtk_window_set_title,
    gtk_window_set_transient_for,
};
use gio_sys::{
    GAsyncReadyCallback, GAsyncResult, GCancellable, GTask, g_task_new, g_task_propagate_boolean,
    g_task_return_boolean,
};
use glib_sys::{GFALSE, GTRUE, GType, g_list_free_full, gpointer};
use gobject_sys::{
    G_TYPE_STRING, GObject, GObjectClass, GTypeInstance, GValue, g_object_ref, g_object_unref,
    g_signal_connect_data, g_type_check_instance_is_a, g_type_from_name, g_value_init,
    g_value_set_string, g_value_unset,
};
use jmap_backend_core::callback::gcallback;
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::i18n::{N_, translate, translate_static, translate_with};
use jmap_backend_core::object_data::ObjectData;
use jmap_backend_core::subclass::{InterfaceDecl, InterfaceImpl, ObjectSubclass, TypeSizes};
use jmap_backend_core::trampoline::{guard, log_critical};
use jmap_proto::Id;

use crate::delegates::io::{self, Loaded};
use crate::delegation::{AddressBookDelegationLevel, CalendarDelegationLevel};
use crate::dispatch;
use crate::link::{self, AccountLink, LoadOutcome, SendSource};
use crate::widget;

const NAME: &CStr = c"JmapDelegatesPage";
const STATE: ObjectData<RefCell<UiState>> = ObjectData::new(c"jmap-delegates-page-state");

/// `GtkScrolledWindow`'s `hscrollbar-policy` value that keeps the page from
/// ever scrolling sideways (`gtkscrolledwindow.h`: ALWAYS = 0, AUTOMATIC = 1,
/// NEVER = 2), the same constant [`crate::vacation::page`] defines.
const GTK_POLICY_NEVER: c_int = 2;

const COLUMN_PRINCIPAL: c_int = 0;
const COLUMN_CALENDAR: c_int = 1;
const COLUMN_BOOK: c_int = 2;

const CONTACTING: &CStr = N_(c"Contacting the mail server…");
const NOT_OFFERED: &CStr =
    N_(c"This account’s server does not offer calendar or address book sharing.");
const NO_DELEGATES: &CStr = N_(c"No delegates yet.");
const OFFLINE: &CStr = N_(c"You are offline. Delegates cannot be read until you reconnect.");
// TRANSLATORS: %1$s is the reason the mail server gave.
const LOAD_FAILED: &CStr = N_(c"Delegates could not be read: %1$s");
// TRANSLATORS: %1$s is the reason the mail server gave.
const REVOKE_FAILED: &CStr = N_(c"The delegate could not be removed: %1$s");
// TRANSLATORS: %1$s is the reason the mail server gave.
const GRANT_FAILED: &CStr = N_(c"The delegate could not be added: %1$s");

const DELEGATE_COLUMN_TITLE: &CStr = N_(c"Delegate");
const CALENDAR_COLUMN_TITLE: &CStr = N_(c"Calendars");
const BOOK_COLUMN_TITLE: &CStr = N_(c"Address Books");

const REVOKE_LABEL: &CStr = N_(c"Re_voke");
const ADD_LABEL: &CStr = N_(c"_Add…");
const ADD_DIALOG_TITLE: &CStr = N_(c"Add Delegate");
const PRINCIPAL_LABEL: &CStr = N_(c"_Principal:");
const CALENDAR_LEVEL_LABEL: &CStr = N_(c"_Calendar access:");
const BOOK_LEVEL_LABEL: &CStr = N_(c"Address _book access:");

/// The calendar level combo's items, in the order it offers them.
const CALENDAR_LEVELS: [CalendarDelegationLevel; 5] = [
    CalendarDelegationLevel::None,
    CalendarDelegationLevel::FreeBusyOnly,
    CalendarDelegationLevel::Reviewer,
    CalendarDelegationLevel::Author,
    CalendarDelegationLevel::Editor,
];

/// The address book level combo's items, in the order it offers them.
const BOOK_LEVELS: [AddressBookDelegationLevel; 4] = [
    AddressBookDelegationLevel::None,
    AddressBookDelegationLevel::Reviewer,
    AddressBookDelegationLevel::Author,
    AddressBookDelegationLevel::Editor,
];

/// The item of `items` a combo's active index names. `gtk_combo_box_get_active`
/// answers -1 for "nothing chosen", which becomes `None` here, the same
/// contract `crate::filters::rule_dialog::item_at` gives its own combos.
fn level_at<T: Copy>(items: &[T], index: c_int) -> Option<T> {
    usize::try_from(index)
        .ok()
        .and_then(|at| items.get(at))
        .copied()
}

/// All page state, in a box as qdata.
struct UiState {
    link: Option<Arc<AccountLink>>,
    status: *mut GtkWidget,
    view: *mut GtkWidget,
    store: *mut GtkListStore,
    revoke: *mut GtkWidget,
    add: *mut GtkWidget,
    /// Parallel to the store's rows: row `i`'s principal id. The selected
    /// row's id comes from here, not from reading the store back, the same
    /// division [`crate::filters::page`]'s own form keeps between the
    /// widgets and the data they show.
    delegates: Vec<Id>,
}

/// The qdata of the page, while the page is alive.
///
/// # Safety
///
/// `page` must be a live `JmapDelegatesPage`. The reference must not live
/// longer than the page (callers use it in one main-loop callback).
unsafe fn state<'a>(page: *mut GObject) -> Option<&'a RefCell<UiState>> {
    // SAFETY: per the contract. instance_init sets the data one time.
    unsafe { STATE.get(page) }
}

pub struct DelegatesPage;

// SAFETY: the parent is GtkScrolledWindow. type_sizes() gets its sizes from
// the running type system. The subclass adds no inline fields (the state is
// qdata). GTypeInstance and GObjectClass are prefixes of all GObject-derived
// structs, so the casts in the trampolines stay in bounds. Same reasoning as
// `crate::vacation::page::VacationPage`.
unsafe impl ObjectSubclass for DelegatesPage {
    const NAME: &'static CStr = NAME;
    type Instance = GTypeInstance;
    type Class = GObjectClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initializes itself.
        unsafe { gtk_scrolled_window_get_type() }
    }

    fn type_sizes() -> TypeSizes {
        TypeSizes::of_parent(Self::parent_type())
    }

    fn interfaces() -> Vec<InterfaceDecl> {
        vec![InterfaceDecl::filled_by::<PageInterface>()]
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // SAFETY: GObject passes a live instance. The parent initialized its
        // scrolled-window part before this call.
        unsafe { build_widgets(instance.cast::<GObject>()) };
    }
}

/// Fills the copy of `EMailConfigPageInterface` for the page.
struct PageInterface;

// SAFETY: as `crate::vacation::page::PageInterface`.
unsafe impl InterfaceImpl for PageInterface {
    type Vtable = EMailConfigPageInterface;

    fn gtype() -> GType {
        // SAFETY: no arguments, and the type initializes itself.
        unsafe { e_mail_config_page_get_type() }
    }

    unsafe fn interface_init(vtable: *mut Self::Vtable) {
        // SAFETY: this is the page copy of the vtable.
        let vtable = unsafe { &mut *vtable };
        vtable.title = translate_static(N_(c"Delegates"));
        // After Server Filters (551); this page writes nothing yet.
        vtable.sort_order = 552;
        vtable.submit = Some(submit);
        vtable.submit_finish = Some(submit_finish);
    }
}

/// Adds one text column that reads `column` of the list store.
///
/// # Safety
///
/// `view` must be a live `GtkTreeView` with a model that has `column`.
unsafe fn add_text_column(view: *mut GtkWidget, title: &'static CStr, column: c_int) {
    // SAFETY: the column and the renderer are new, and the view takes the
    // column, which takes the renderer.
    unsafe {
        let renderer: *mut GtkCellRenderer = gtk_cell_renderer_text_new();
        let tree_column = gtk_tree_view_column_new();
        gtk_tree_view_column_set_title(tree_column, translate_static(title));
        gtk_tree_view_column_pack_start(tree_column, renderer, GTRUE);
        gtk_tree_view_column_add_attribute(tree_column, renderer, c"text".as_ptr(), column);
        gtk_tree_view_append_column(view.cast(), tree_column);
    }
}

/// Appends one row of three string columns to the list store.
///
/// # Safety
///
/// `store` must be the live three-string-column store of a live page.
unsafe fn append_row(store: *mut GtkListStore, principal: &str, calendar: &str, book: &str) {
    let principal = cstring_lossy(principal);
    let calendar = cstring_lossy(calendar);
    let book = cstring_lossy(book);
    let mut columns = [COLUMN_PRINCIPAL, COLUMN_CALENDAR, COLUMN_BOOK];
    // SAFETY: a zeroed GValue is `G_VALUE_INIT`, the state `g_value_init`
    // requires. All three values are strings, which the store's three
    // columns hold, and all three are released below. NULL as the iterator
    // means "do not report the new row back", which this caller does not
    // need.
    unsafe {
        let mut values: [GValue; 3] = [std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed()];
        for (value, text) in values.iter_mut().zip([&principal, &calendar, &book]) {
            g_value_init(value, G_TYPE_STRING);
            g_value_set_string(value, text.as_ptr());
        }
        gtk_list_store_insert_with_valuesv(
            store,
            ptr::null_mut(),
            -1,
            columns.as_mut_ptr(),
            values.as_mut_ptr(),
            3,
        );
        for value in values.iter_mut() {
            g_value_unset(value);
        }
    }
}

/// Rewrites the list from `loaded`, including the parallel `delegates`
/// principal-id vector [`on_revoke`] reads the selected row from, and sets
/// the status label and the list's sensitivity. Used both after the initial
/// load and after a successful [`on_revoke`] round trip.
///
/// # Safety
///
/// `cell` must be the state of a live page, on the main thread. No borrow of
/// it may be held.
unsafe fn fill_list(cell: &RefCell<UiState>, loaded: &Loaded) {
    let (status, store, view, add) = {
        let ui = cell.borrow();
        (ui.status, ui.store, ui.view, ui.add)
    };
    // SAFETY: these are live children of the page. No borrow of `cell` is
    // held during these calls.
    unsafe {
        gtk_list_store_clear(store);
        for delegate in &loaded.delegates {
            append_row(
                store,
                delegate.principal_id.as_str(),
                &calendar_level_label(delegate.calendar_level),
                &address_book_level_label(delegate.address_book_level),
            );
        }
        let text = if loaded.delegates.is_empty() {
            cstring_lossy(&translate(NO_DELEGATES))
        } else {
            cstring_lossy("")
        };
        gtk_label_set_text(status.cast(), text.as_ptr());
        gtk_widget_set_sensitive(view, GTRUE);
        gtk_widget_set_sensitive(add, GTRUE);
    }
    cell.borrow_mut().delegates = loaded
        .delegates
        .iter()
        .map(|delegate| delegate.principal_id.clone())
        .collect();
}

/// The text a calendar delegation level shows in its column.
fn calendar_level_label(level: CalendarDelegationLevel) -> String {
    translate(match level {
        CalendarDelegationLevel::None => N_(c"No access"),
        CalendarDelegationLevel::FreeBusyOnly => N_(c"Free/busy only"),
        CalendarDelegationLevel::Reviewer => N_(c"Reviewer"),
        CalendarDelegationLevel::Author => N_(c"Author"),
        CalendarDelegationLevel::Editor => N_(c"Editor"),
    })
}

/// The text an address book delegation level shows in its column.
fn address_book_level_label(level: AddressBookDelegationLevel) -> String {
    translate(match level {
        AddressBookDelegationLevel::None => N_(c"No access"),
        AddressBookDelegationLevel::Reviewer => N_(c"Reviewer"),
        AddressBookDelegationLevel::Author => N_(c"Author"),
        AddressBookDelegationLevel::Editor => N_(c"Editor"),
    })
}

/// Builds the only child of the page (a vertical box with the widgets) and
/// stores the [`UiState`]. The list starts insensitive and empty. The load
/// fills it, or shows why not in the status label.
///
/// # Safety
///
/// `page` must be a freshly initialised `JmapDelegatesPage` instance.
unsafe fn build_widgets(page: *mut GObject) {
    // SAFETY: the instance is a GtkScrolledWindow that the parent
    // initialized, as the container cast requires. GTK3 adds a viewport
    // around the box. Each widget below is new, and its container owns it.
    unsafe {
        widget::set_enum_named(page, c"hscrollbar-policy", GTK_POLICY_NEVER);

        let content = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        gtk_container_add(page.cast(), content);

        let status = gtk_label_new(ptr::null());
        gtk_label_set_xalign(status.cast(), 0.0);
        widget::set_bool_named(status.cast(), c"wrap", true);
        widget::set_enum_named(status.cast(), c"wrap-mode", widget::PANGO_WRAP_WORD_CHAR);
        // A screen reader otherwise stays silent on a later `set_text`: a
        // status role tells it this is an announcement, not a redraw.
        widget::set_accessible_role(status.cast(), evo_sys::ATK_ROLE_STATUSBAR);
        gtk_box_pack_start(content.cast(), status, GFALSE, GFALSE, 0);
        let contacting = cstring_lossy(&translate(CONTACTING));
        gtk_label_set_text(status.cast(), contacting.as_ptr());

        let mut column_types = [G_TYPE_STRING, G_TYPE_STRING, G_TYPE_STRING];
        let store = gtk_list_store_newv(3, column_types.as_mut_ptr());
        let view = gtk_tree_view_new_with_model(store.cast::<GtkTreeModel>());
        // SAFETY: the view owns the only remaining reference once this one
        // is released, the same pattern as the filters page's script list.
        g_object_unref(store.cast());
        gtk_tree_view_set_headers_visible(view.cast(), GTRUE);
        widget::set_accessible_name(view.cast(), &translate(DELEGATE_COLUMN_TITLE));
        add_text_column(view, DELEGATE_COLUMN_TITLE, COLUMN_PRINCIPAL);
        add_text_column(view, CALENDAR_COLUMN_TITLE, COLUMN_CALENDAR);
        add_text_column(view, BOOK_COLUMN_TITLE, COLUMN_BOOK);
        gtk_widget_set_hexpand(view, GTRUE);
        gtk_widget_set_vexpand(view, GTRUE);
        gtk_widget_set_sensitive(view, GFALSE);
        gtk_box_pack_start(content.cast(), view, GTRUE, GTRUE, 0);

        let selection = gtk_tree_view_get_selection(view.cast());
        gtk_tree_selection_set_mode(selection, GTK_SELECTION_BROWSE);

        let buttons = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        gtk_box_pack_start(content.cast(), buttons, GFALSE, GFALSE, 0);

        let add = gtk_button_new_with_mnemonic(translate_static(ADD_LABEL));
        gtk_box_pack_start(buttons.cast(), add, GFALSE, GFALSE, 0);
        gtk_widget_set_sensitive(add, GFALSE);

        let revoke = gtk_button_new_with_mnemonic(translate_static(REVOKE_LABEL));
        gtk_box_pack_start(buttons.cast(), revoke, GFALSE, GFALSE, 0);
        gtk_widget_set_sensitive(revoke, GFALSE);

        gtk_widget_show_all(page.cast());

        STATE.set(
            page,
            RefCell::new(UiState {
                link: None,
                status,
                view,
                store,
                revoke,
                add,
                delegates: Vec::new(),
            }),
        );

        g_signal_connect_data(
            selection.cast(),
            c"changed".as_ptr(),
            gcallback(on_selection_changed as unsafe extern "C" fn(*mut GObject, gpointer)),
            page.cast(),
            None,
            0,
        );
        g_signal_connect_data(
            revoke.cast(),
            c"clicked".as_ptr(),
            gcallback(on_revoke as unsafe extern "C" fn(*mut GObject, gpointer)),
            page.cast(),
            None,
            0,
        );
        g_signal_connect_data(
            add.cast(),
            c"clicked".as_ptr(),
            gcallback(on_add as unsafe extern "C" fn(*mut GObject, gpointer)),
            page.cast(),
            None,
            0,
        );
    }
}

/// The row a `GtkTreePath` from the selection names.
///
/// # Safety
///
/// `path` must be a live path of the list store's model.
unsafe fn row_of(path: *mut GtkTreePath) -> Option<usize> {
    let mut depth: c_int = 0;
    // SAFETY: the path is live, per the contract. The returned array belongs
    // to the path and has `depth` entries.
    let indices = unsafe { gtk_tree_path_get_indices_with_depth(path, &mut depth) };
    if indices.is_null() || depth < 1 {
        return None;
    }
    // SAFETY: `depth` is at least one, so the first entry is in bounds. A
    // list store is one level deep, so that entry is the row.
    let row = unsafe { *indices };
    usize::try_from(row).ok()
}

/// Frees one `GtkTreePath` of the list that
/// `gtk_tree_selection_get_selected_rows` returns.
///
/// # Safety
///
/// `data` must be a `GtkTreePath *` the caller owns.
unsafe extern "C" fn free_tree_path(data: gpointer) {
    // SAFETY: per the contract.
    unsafe { gtk_tree_path_free(data.cast::<GtkTreePath>()) };
}

/// The selected row, or `None` when nothing is selected.
///
/// # Safety
///
/// `selection` must be the live selection of the page's list.
unsafe fn selected_row(selection: *mut GtkTreeSelection) -> Option<usize> {
    let mut model: *mut GtkTreeModel = ptr::null_mut();
    // SAFETY: the selection is live. The caller owns the list and every path
    // in it, which the free below releases together.
    unsafe {
        let rows = gtk_tree_selection_get_selected_rows(selection, &mut model);
        if rows.is_null() {
            return None;
        }
        let first = (*rows).data.cast::<GtkTreePath>();
        let row = (!first.is_null()).then(|| row_of(first)).flatten();
        g_list_free_full(rows, Some(free_tree_path));
        row
    }
}

/// Sets the Revoke button's sensitivity from whether a row is selected and
/// the account is reachable (a live `link`, the same gate every write in
/// this crate uses).
///
/// # Safety
///
/// `cell` must be the state of a live page, on the main thread. No borrow
/// of it may be held.
unsafe fn update_revoke_sensitivity(cell: &RefCell<UiState>) {
    let (revoke, selected, has_link) = {
        let ui = cell.borrow();
        // SAFETY: the view is a live child of the page.
        let selected = unsafe { selected_row(gtk_tree_view_get_selection(ui.view.cast())) };
        (ui.revoke, selected, ui.link.is_some())
    };
    // SAFETY: the button is a live child of the page.
    unsafe { gtk_widget_set_sensitive(revoke, flag(selected.is_some() && has_link)) };
}

/// `GFALSE`/`GTRUE` for a `bool`.
fn flag(value: bool) -> glib_sys::gboolean {
    if value { GTRUE } else { GFALSE }
}

/// The `changed` handler of the list selection: only the Revoke button's
/// sensitivity depends on it, so this never touches the store.
///
/// # Safety
///
/// The GLib signal system calls this. `page` is the data pointer of the
/// connection: the page the selection belongs to.
unsafe extern "C" fn on_selection_changed(_selection: *mut GObject, page: gpointer) {
    guard("JmapDelegatesPage::selection-changed", (), || {
        // SAFETY: the page is live, because its descendant emitted.
        let Some(cell) = (unsafe { state(page.cast()) }) else {
            return;
        };
        // SAFETY: as above.
        unsafe { update_revoke_sensitivity(cell) };
    });
}

/// The `clicked` handler of the Revoke button: clears the selected
/// delegate's access and reloads the list, both on a worker thread (the
/// same `dispatch::spawn_for` shape [`create`] uses for the initial load).
///
/// # Safety
///
/// The GLib signal system calls this. `page` is the data pointer of the
/// connection: the page the button belongs to.
unsafe extern "C" fn on_revoke(_button: *mut GObject, page: gpointer) {
    guard("JmapDelegatesPage::revoke", (), || {
        // SAFETY: the page is live, because its descendant emitted.
        let Some(cell) = (unsafe { state(page.cast()) }) else {
            return;
        };
        let (link, principal_id, view, revoke, add) = {
            let ui = cell.borrow();
            let Some(link) = ui.link.clone() else {
                return;
            };
            // SAFETY: the view is a live child of the page.
            let Some(row) = (unsafe { selected_row(gtk_tree_view_get_selection(ui.view.cast())) })
            else {
                return;
            };
            let Some(principal_id) = ui.delegates.get(row).cloned() else {
                return;
            };
            (link, principal_id, ui.view, ui.revoke, ui.add)
        };
        // SAFETY: all three are live children of the page.
        unsafe {
            gtk_widget_set_sensitive(view, GFALSE);
            gtk_widget_set_sensitive(revoke, GFALSE);
            gtk_widget_set_sensitive(add, GFALSE);
        }
        let work = move || io::revoke(&link, &principal_id);
        let finish = |page: *mut GObject, outcome: Result<Loaded, String>| {
            // SAFETY: `with_strong` gives the live page.
            unsafe { apply_write_outcome(page, outcome, REVOKE_FAILED) };
        };
        // SAFETY: `page` is the live page handling this click.
        unsafe { dispatch::spawn_for(page.cast(), work, finish) };
    });
}

/// Builds the Add dialog, unshown and not yet run. Returns the dialog, the
/// principal entry and the calendar and address book level combo boxes.
/// Split out from [`ask_for_grant`] so a test can check the dialog's
/// keyboard behaviour without running its nested main loop, the same split
/// `crate::filters::page::build_name_dialog`/`ask_for_name` uses.
///
/// # Safety
///
/// `page` must be the live page, on the main thread.
unsafe fn build_add_dialog(
    page: *mut GObject,
) -> (
    *mut GtkWidget,
    *mut GtkWidget,
    *mut GtkWidget,
    *mut GtkWidget,
) {
    // SAFETY: the page is live, per the contract. Every widget is new, and
    // the caller owns the returned dialog.
    unsafe {
        let dialog = gtk_dialog_new();
        gtk_window_set_title(dialog.cast(), translate_static(ADD_DIALOG_TITLE));
        let toplevel = gtk_widget_get_toplevel(page.cast());
        // As `build_name_dialog`: before the editor packs the page, its
        // toplevel is the page itself, which is no window.
        if !toplevel.is_null()
            && g_type_check_instance_is_a(toplevel.cast::<GTypeInstance>(), gtk_window_get_type())
                != GFALSE
        {
            gtk_window_set_transient_for(dialog.cast(), toplevel.cast::<GtkWindow>());
        }
        gtk_dialog_add_button(
            dialog.cast(),
            translate_static(N_(c"_Cancel")),
            GTK_RESPONSE_CANCEL,
        );
        gtk_dialog_add_button(dialog.cast(), translate_static(ADD_LABEL), GTK_RESPONSE_OK);
        gtk_dialog_set_default_response(dialog.cast(), GTK_RESPONSE_OK);

        let content_area = gtk_dialog_get_content_area(dialog.cast());
        // GNOME HIG: 12px between a dialog's content and its edges.
        gtk_container_set_border_width(content_area.cast(), 12);
        let content = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        gtk_container_add(content_area.cast(), content);

        let principal_row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        let principal_label = gtk_label_new_with_mnemonic(translate_static(PRINCIPAL_LABEL));
        let principal_entry = gtk_entry_new();
        gtk_entry_set_activates_default(principal_entry.cast(), GTRUE);
        gtk_label_set_mnemonic_widget(principal_label.cast(), principal_entry);
        gtk_box_pack_start(principal_row.cast(), principal_label, GFALSE, GFALSE, 0);
        gtk_widget_set_hexpand(principal_entry, GTRUE);
        gtk_box_pack_start(principal_row.cast(), principal_entry, GTRUE, GTRUE, 0);
        gtk_box_pack_start(content.cast(), principal_row, GFALSE, GFALSE, 0);

        let calendar_row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        let calendar_label = gtk_label_new_with_mnemonic(translate_static(CALENDAR_LEVEL_LABEL));
        let calendar_combo = gtk_combo_box_text_new();
        for level in CALENDAR_LEVELS {
            let text = cstring_lossy(&calendar_level_label(level));
            gtk_combo_box_text_append(calendar_combo.cast(), ptr::null(), text.as_ptr());
        }
        gtk_combo_box_set_active(calendar_combo.cast(), 0);
        gtk_label_set_mnemonic_widget(calendar_label.cast(), calendar_combo);
        gtk_box_pack_start(calendar_row.cast(), calendar_label, GFALSE, GFALSE, 0);
        gtk_widget_set_hexpand(calendar_combo, GTRUE);
        gtk_box_pack_start(calendar_row.cast(), calendar_combo, GTRUE, GTRUE, 0);
        gtk_box_pack_start(content.cast(), calendar_row, GFALSE, GFALSE, 0);

        let book_row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        let book_label = gtk_label_new_with_mnemonic(translate_static(BOOK_LEVEL_LABEL));
        let book_combo = gtk_combo_box_text_new();
        for level in BOOK_LEVELS {
            let text = cstring_lossy(&address_book_level_label(level));
            gtk_combo_box_text_append(book_combo.cast(), ptr::null(), text.as_ptr());
        }
        gtk_combo_box_set_active(book_combo.cast(), 0);
        gtk_label_set_mnemonic_widget(book_label.cast(), book_combo);
        gtk_box_pack_start(book_row.cast(), book_label, GFALSE, GFALSE, 0);
        gtk_widget_set_hexpand(book_combo, GTRUE);
        gtk_box_pack_start(book_row.cast(), book_combo, GTRUE, GTRUE, 0);
        gtk_box_pack_start(content.cast(), book_row, GFALSE, GFALSE, 0);

        (dialog, principal_entry, calendar_combo, book_combo)
    }
}

/// Asks for a new delegate's principal id and the two access levels to
/// grant it, in a modal dialog. `None` means the user cancelled, or left
/// the principal field empty.
///
/// # Safety
///
/// `page` must be the live page, on the main thread.
unsafe fn ask_for_grant(
    page: *mut GObject,
) -> Option<(Id, CalendarDelegationLevel, AddressBookDelegationLevel)> {
    // SAFETY: the page is live, per the contract. The dialog owns its
    // children, and this function destroys the dialog.
    unsafe {
        let (dialog, entry, calendar_combo, book_combo) = build_add_dialog(page);
        gtk_widget_show_all(dialog);

        let response = gtk_dialog_run(dialog.cast());
        let result = (response == GTK_RESPONSE_OK)
            .then(|| widget::get_text(entry.cast()))
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
            .map(|principal| {
                let calendar_level = level_at(
                    &CALENDAR_LEVELS,
                    gtk_combo_box_get_active(calendar_combo.cast()),
                )
                .unwrap_or(CalendarDelegationLevel::None);
                let book_level =
                    level_at(&BOOK_LEVELS, gtk_combo_box_get_active(book_combo.cast()))
                        .unwrap_or(AddressBookDelegationLevel::None);
                (Id::new(principal), calendar_level, book_level)
            });
        gtk_widget_destroy(dialog);
        result
    }
}

/// The `clicked` handler of the Add button: asks for a principal and the
/// levels to grant it, then writes and reloads the list, the same worker
/// shape [`on_revoke`] uses. Asking is a nested main loop; nothing here
/// disables a widget until the user confirms, so Cancel leaves the page
/// exactly as it was.
///
/// # Safety
///
/// The GLib signal system calls this. `page` is the data pointer of the
/// connection: the page the button belongs to.
unsafe extern "C" fn on_add(_button: *mut GObject, page: gpointer) {
    guard("JmapDelegatesPage::add", (), || {
        // SAFETY: the page is live, because its descendant emitted.
        let Some(cell) = (unsafe { state(page.cast()) }) else {
            return;
        };
        let Some(link) = cell.borrow().link.clone() else {
            return;
        };
        // SAFETY: the page is live, and this runs on the main thread, per
        // the signal handler contract.
        let Some((principal_id, calendar_level, book_level)) =
            (unsafe { ask_for_grant(page.cast()) })
        else {
            return;
        };
        let (view, revoke, add) = {
            let ui = cell.borrow();
            (ui.view, ui.revoke, ui.add)
        };
        // SAFETY: all three are live children of the page.
        unsafe {
            gtk_widget_set_sensitive(view, GFALSE);
            gtk_widget_set_sensitive(revoke, GFALSE);
            gtk_widget_set_sensitive(add, GFALSE);
        }
        let work = move || io::grant(&link, &principal_id, calendar_level, book_level);
        let finish = |page: *mut GObject, outcome: Result<Loaded, String>| {
            // SAFETY: `with_strong` gives the live page.
            unsafe { apply_write_outcome(page, outcome, GRANT_FAILED) };
        };
        // SAFETY: `page` is the live page handling this click.
        unsafe { dispatch::spawn_for(page.cast(), work, finish) };
    });
}

/// Applies the result of [`on_revoke`]'s or [`on_add`]'s round trip: the
/// fresh list on success, or `failed` (with the server's reason) on the
/// status label otherwise. Either way, the view, Add and Revoke become
/// usable again.
///
/// # Safety
///
/// `page` must be the live page of the write.
unsafe fn apply_write_outcome(page: *mut GObject, outcome: Result<Loaded, String>, failed: &CStr) {
    let Some(cell) = (unsafe { state(page.cast()) }) else {
        return;
    };
    match outcome {
        Ok(loaded) => {
            // SAFETY: no borrow of `cell` is held across this call.
            unsafe { fill_list(cell, &loaded) };
        }
        Err(message) => {
            let text = translate_with(failed, &[&message]);
            let text = cstring_lossy(&text);
            let status = cell.borrow().status;
            // SAFETY: the status label is a live child of the page.
            unsafe { gtk_label_set_text(status.cast(), text.as_ptr()) };
        }
    }
    let (view, add) = {
        let ui = cell.borrow();
        (ui.view, ui.add)
    };
    // SAFETY: both are live children of the page. The Revoke button's own
    // sensitivity depends on the (now cleared) selection, not a flat GTRUE.
    unsafe {
        gtk_widget_set_sensitive(view, GTRUE);
        gtk_widget_set_sensitive(add, GTRUE);
        update_revoke_sensitivity(cell);
    }
}

/// Makes the registered page for the account of `connect_source` and starts
/// its load, as [`crate::vacation::page::create`] does.
///
/// # Safety
///
/// `connect_source` must be a valid `ESource`. The function keeps a reference
/// to it during the load.
pub unsafe fn create(connect_source: *mut ESource) -> *mut EMailConfigPage {
    // SAFETY: NAME is a 'static NUL-terminated string.
    let gtype = unsafe { g_type_from_name(NAME.as_ptr()) };
    if gtype == 0 {
        log_critical("JmapDelegatesPage: not registered; was the module loaded?");
        return ptr::null_mut();
    }
    // SAFETY: the widget type is registered. The new GtkWidget is floating,
    // and the container of e_mail_config_notebook_add_page sinks it.
    let page = unsafe {
        gobject_sys::g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null())
    };
    if page.is_null() {
        return ptr::null_mut();
    }

    // SAFETY: `create` runs on the main loop, per its own contract (it builds
    // a GTK widget above).
    let offline = unsafe { link::shell_is_offline() };
    // SAFETY: the source is valid, per the contract of this function. The
    // worker owns the reference and releases it.
    let source = unsafe { SendSource::adopt(g_object_ref(connect_source.cast()).cast()) };
    let work = move || {
        let outcome = if offline {
            LoadOutcome::Offline
        } else {
            // SAFETY: the reference from `create` keeps the source alive for
            // all of this closure.
            match unsafe { link::connect_account(source.as_ptr()) }
                .and_then(|link| io::load(&link).map(|loaded| (Arc::new(link), loaded)))
            {
                Ok(loaded) => LoadOutcome::Loaded(loaded),
                Err(message) => LoadOutcome::Failed(message),
            }
        };
        drop(source);
        outcome
    };
    let finish = |page: *mut GObject, outcome| {
        // SAFETY: `with_strong` gives the live page.
        unsafe { apply_outcome(page, outcome) };
    };
    // SAFETY: `page` is the live new instance, and this function holds its reference.
    unsafe { dispatch::spawn_for(page, work, finish) };

    page.cast()
}

/// Fills the list with the load result, or shows why the load failed. The
/// weak reference of dispatch makes sure that this runs on the main loop
/// and that the page is alive.
///
/// # Safety
///
/// `page` must be the live page of the load.
unsafe fn apply_outcome(page: *mut GObject, outcome: LoadOutcome<(Arc<AccountLink>, Loaded)>) {
    let Some(cell) = (unsafe { state(page) }) else {
        return;
    };
    match outcome {
        LoadOutcome::Loaded((link, loaded)) if loaded.offered => {
            {
                let mut ui = cell.borrow_mut();
                ui.link = Some(link);
            }
            // SAFETY: no borrow of `cell` is held across this call.
            unsafe { fill_list(cell, &loaded) };
        }
        LoadOutcome::Loaded(_) => {
            let text = cstring_lossy(&translate(NOT_OFFERED));
            let status = cell.borrow().status;
            // SAFETY: the status label is a live child of the page.
            unsafe { gtk_label_set_text(status.cast(), text.as_ptr()) };
        }
        LoadOutcome::Offline => {
            let text = cstring_lossy(&translate(OFFLINE));
            let status = cell.borrow().status;
            // SAFETY: as above.
            unsafe { gtk_label_set_text(status.cast(), text.as_ptr()) };
        }
        LoadOutcome::Failed(message) => {
            let text = translate_with(LOAD_FAILED, &[&message]);
            let text = cstring_lossy(&text);
            let status = cell.borrow().status;
            // SAFETY: as above.
            unsafe { gtk_label_set_text(status.cast(), text.as_ptr()) };
        }
    }
}

/// `EMailConfigPage.submit`: the save button of the account editor. This
/// page writes nothing yet (it is read-only), so it always succeeds without
/// a server call.
///
/// # Safety
///
/// The interface vtable calls this with a live page. `cancellable` is NULL
/// or valid. `callback` and `user_data` are the usual GIO pair.
unsafe extern "C" fn submit(
    page: *mut EMailConfigPage,
    cancellable: *mut GCancellable,
    callback: GAsyncReadyCallback,
    user_data: gpointer,
) {
    guard("JmapDelegatesPage::submit", (), || {
        // SAFETY: the page is live, per the vfunc contract.
        let task = unsafe { g_task_new(page.cast(), cancellable, callback, user_data) };
        // SAFETY: completes the new task and releases the local reference.
        unsafe {
            g_task_return_boolean(task, GTRUE);
            g_object_unref(task.cast());
        }
    });
}

/// `EMailConfigPage.submit_finish`.
///
/// # Safety
///
/// The interface vtable calls this. `result` is the `GTask` that [`submit`]
/// made for this page. `error` is NULL or writable.
unsafe extern "C" fn submit_finish(
    _page: *mut EMailConfigPage,
    result: *mut GAsyncResult,
    error: *mut *mut glib_sys::GError,
) -> glib_sys::gboolean {
    guard("JmapDelegatesPage::submit_finish", GFALSE, || {
        // SAFETY: `result` is the task from submit. The GIO contract gives
        // finish the same result that the callback received.
        unsafe { g_task_propagate_boolean(result.cast::<GTask>(), error) }
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::{c_char, c_int, c_void};

    use gobject_sys::g_object_get;
    use jmap_backend_core::marshal::read_string;

    use super::*;

    #[test]
    fn level_at_reads_back_every_valid_index() {
        for (index, level) in CALENDAR_LEVELS.iter().enumerate() {
            assert_eq!(level_at(&CALENDAR_LEVELS, index as c_int), Some(*level));
        }
    }

    #[test]
    fn level_at_is_none_for_no_selection_or_an_out_of_range_index() {
        assert_eq!(level_at(&CALENDAR_LEVELS, -1), None, "no selection");
        assert_eq!(
            level_at(&CALENDAR_LEVELS, CALENDAR_LEVELS.len() as c_int),
            None,
            "one past the end"
        );
    }

    #[test]
    fn calendar_levels_each_have_their_own_label() {
        let labels: Vec<String> = [
            CalendarDelegationLevel::None,
            CalendarDelegationLevel::FreeBusyOnly,
            CalendarDelegationLevel::Reviewer,
            CalendarDelegationLevel::Author,
            CalendarDelegationLevel::Editor,
        ]
        .into_iter()
        .map(calendar_level_label)
        .collect();
        let mut unique = labels.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), labels.len(), "every level needs its own text");
    }

    #[test]
    fn address_book_levels_each_have_their_own_label() {
        let labels: Vec<String> = [
            AddressBookDelegationLevel::None,
            AddressBookDelegationLevel::Reviewer,
            AddressBookDelegationLevel::Author,
        ]
        .into_iter()
        .map(address_book_level_label)
        .collect();
        let mut unique = labels.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), labels.len(), "every level needs its own text");
    }

    unsafe extern "C" {
        fn gtk_init_check(argc: *mut c_int, argv: *mut *mut *mut c_char) -> glib_sys::gboolean;
        fn gtk_scrolled_window_new(
            hadjustment: *mut c_void,
            vadjustment: *mut c_void,
        ) -> *mut GtkWidget;
    }

    /// Layout and accessibility: the list starts insensitive, contacting
    /// status shown, three named columns, no sideways scrolling, the same
    /// shape `crate::vacation::page`'s own first test checks. Needs a
    /// display; skips without one.
    #[test]
    fn the_list_starts_insensitive_with_three_named_columns() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        // SAFETY: a plain `GtkScrolledWindow` stands in for the registered
        // `JmapDelegatesPage` instance; `build_widgets` only uses it as a
        // `GtkContainer` and a qdata target, both of which any `GObject`
        // instance supports.
        unsafe {
            let page = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
            build_widgets(page.cast());

            let mut hscrollbar_policy: c_int = -1;
            g_object_get(
                page.cast(),
                c"hscrollbar-policy".as_ptr(),
                &mut hscrollbar_policy as *mut c_int,
                ptr::null::<c_char>(),
            );
            assert_eq!(
                hscrollbar_policy, GTK_POLICY_NEVER,
                "the page can still scroll sideways"
            );

            let ui = state(page.cast()).unwrap().borrow();
            let text =
                read_string(evo_sys::gtk_label_get_text(ui.status.cast())).unwrap_or_default();
            assert_eq!(text, translate(CONTACTING));

            assert!(!evo_sys::gtk_tree_view_get_column(ui.view.cast(), 0).is_null());
            assert!(!evo_sys::gtk_tree_view_get_column(ui.view.cast(), 1).is_null());
            assert!(!evo_sys::gtk_tree_view_get_column(ui.view.cast(), 2).is_null());
            assert!(
                evo_sys::gtk_tree_view_get_column(ui.view.cast(), 3).is_null(),
                "the list has more than three columns"
            );
            assert_eq!(
                widget::get_accessible_name(ui.view.cast()),
                translate(DELEGATE_COLUMN_TITLE)
            );

            let mut sensitive: glib_sys::gboolean = GTRUE;
            g_object_get(
                ui.view.cast(),
                c"sensitive".as_ptr(),
                &mut sensitive as *mut glib_sys::gboolean,
                ptr::null::<c_char>(),
            );
            assert_eq!(
                sensitive, GFALSE,
                "the list is usable before the load answers"
            );
            assert!(
                !get_sensitive(ui.revoke),
                "Revoke is usable before any row is selected"
            );
            assert!(
                !get_sensitive(ui.add),
                "Add is usable before the load answers"
            );

            drop(ui);
            gtk_widget_destroy(page);
        }
    }

    /// `widget`'s `sensitive` property.
    ///
    /// # Safety
    ///
    /// `widget` must be a live `GObject` with a `sensitive` property.
    unsafe fn get_sensitive(widget: *mut GtkWidget) -> bool {
        let mut sensitive: glib_sys::gboolean = GTRUE;
        // SAFETY: per the contract.
        unsafe {
            g_object_get(
                widget.cast(),
                c"sensitive".as_ptr(),
                &mut sensitive as *mut glib_sys::gboolean,
                ptr::null::<c_char>(),
            );
        }
        sensitive == GTRUE
    }

    /// The offline message, not a generic load-failure sentence: the same
    /// check `crate::vacation::page`'s own test makes. Needs a display;
    /// skips without one.
    #[test]
    fn offline_shows_an_offline_message_not_a_generic_load_failure() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        // SAFETY: see the test above; the same stand-in instance works here.
        // No network call happens: `LoadOutcome::Offline` is applied directly.
        unsafe {
            let page = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
            build_widgets(page.cast());

            apply_outcome(page.cast(), LoadOutcome::Offline);

            let status = state(page.cast()).unwrap().borrow().status;
            let text = read_string(evo_sys::gtk_label_get_text(status.cast())).unwrap_or_default();
            assert_eq!(
                text,
                translate(OFFLINE),
                "the status label does not say the account is offline"
            );

            gtk_widget_destroy(page);
        }
    }

    /// Selecting a delegate's row makes Revoke usable; the button is
    /// insensitive again as soon as the list starts a fresh load, before
    /// anything is selected in it. Needs a display; skips without one.
    #[test]
    fn selecting_a_delegate_enables_revoke() {
        use jmap_client::{Client, Credentials};
        use jmap_mock::MockServer;

        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        let server = MockServer::builder().start();
        let client = Client::connect(server.origin(), Credentials::none()).unwrap();
        let features =
            crate::session_cache::AccountFeatures::from_session(client.session()).unwrap();
        let link = Arc::new(AccountLink::without_refresh(client, features));
        let loaded = Loaded {
            delegates: vec![crate::delegation::Delegate {
                principal_id: Id::new("P-bob"),
                calendar_level: CalendarDelegationLevel::Reviewer,
                address_book_level: AddressBookDelegationLevel::None,
            }],
            offered: true,
        };

        // SAFETY: as the test above; no network call happens here, the
        // mock client above never makes one outside its own constructor.
        unsafe {
            let page = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
            build_widgets(page.cast());

            apply_outcome(page.cast(), LoadOutcome::Loaded((link, loaded)));

            let (view, revoke) = {
                let ui = state(page.cast()).unwrap().borrow();
                (ui.view, ui.revoke)
            };
            assert!(
                !get_sensitive(revoke),
                "Revoke is usable before any row is selected"
            );

            // Moving the list's cursor to row 0 also selects it (single
            // selection mode), which emits "changed" on the selection
            // synchronously, the same way a real click does.
            let path = evo_sys::gtk_tree_path_new_from_indicesv([0].as_mut_ptr(), 1);
            evo_sys::gtk_tree_view_set_cursor(view.cast(), path, ptr::null_mut(), GFALSE);
            evo_sys::gtk_tree_path_free(path);

            assert!(
                get_sensitive(revoke),
                "Revoke stays insensitive after a row is selected"
            );

            gtk_widget_destroy(page);
        }
    }

    /// A successful load makes Add usable too, not only the list, so a user
    /// can grant a first delegate on an account that has none yet. Needs a
    /// display; skips without one.
    #[test]
    fn loading_delegates_enables_add() {
        use jmap_client::{Client, Credentials};
        use jmap_mock::MockServer;

        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        let server = MockServer::builder().start();
        let client = Client::connect(server.origin(), Credentials::none()).unwrap();
        let features =
            crate::session_cache::AccountFeatures::from_session(client.session()).unwrap();
        let link = Arc::new(AccountLink::without_refresh(client, features));
        let loaded = Loaded {
            delegates: vec![],
            offered: true,
        };

        // SAFETY: as the test above; no network call happens here.
        unsafe {
            let page = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
            build_widgets(page.cast());

            let add = state(page.cast()).unwrap().borrow().add;
            assert!(!get_sensitive(add), "Add is usable before the load answers");

            apply_outcome(page.cast(), LoadOutcome::Loaded((link, loaded)));

            assert!(
                get_sensitive(add),
                "Add stays insensitive after a load with no delegates"
            );

            gtk_widget_destroy(page);
        }
    }

    /// The Add dialog offers "No access" first for both levels (the
    /// combo's own order, [`CALENDAR_LEVELS`]/[`BOOK_LEVELS`] index 0), so
    /// a user who only wants one kind of access does not have to touch the
    /// other combo. The principal entry starts empty and activates the
    /// default response, the same as the name dialog's entry. Needs a
    /// display; skips without one.
    #[test]
    fn the_add_dialog_defaults_to_no_access_for_both_levels() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        // SAFETY: a plain `GtkScrolledWindow` stands in for the registered
        // `JmapDelegatesPage` instance, the same stand-in the other widget
        // tests in this file use; `build_add_dialog` only uses it as the
        // source of `gtk_widget_get_toplevel`.
        unsafe {
            let page = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
            let (dialog, entry, calendar_combo, book_combo) = build_add_dialog(page.cast());

            assert_eq!(
                widget::get_text(entry.cast()),
                "",
                "the principal entry is not empty by default"
            );
            let mut activates_default: glib_sys::gboolean = GFALSE;
            g_object_get(
                entry.cast(),
                c"activates-default".as_ptr(),
                &mut activates_default as *mut glib_sys::gboolean,
                ptr::null::<c_char>(),
            );
            assert_eq!(
                activates_default, GTRUE,
                "Enter in the principal entry does not confirm the dialog"
            );
            assert_eq!(
                gtk_combo_box_get_active(calendar_combo.cast()),
                0,
                "the calendar level combo does not default to No access"
            );
            assert_eq!(
                gtk_combo_box_get_active(book_combo.cast()),
                0,
                "the address book level combo does not default to No access"
            );

            gtk_widget_destroy(dialog);
            gtk_widget_destroy(page);
        }
    }
}
