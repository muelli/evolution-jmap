// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapFiltersPage`: the Server Filters page of the account editor. It is a
//! `GtkScrolledWindow` subclass that implements `EMailConfigPage`, as
//! [`crate::vacation::page`] is, and it follows that page in every structural
//! decision. Read that file first: it explains the parent choice, the qdata
//! state box, the asynchronous load and the `GTask` submit.
//!
//! This page shows the account's Sieve scripts as a list, and the source of
//! the selected one in a text view. Everything the user does is staged in a
//! [`FiltersForm`] and written when the account editor saves. "Check" is the
//! one exception: it asks the server at once, and writes nothing.
//!
//! The widgets hold no decisions. The form answers which buttons are usable
//! and what one submit writes, and it has unit tests. This file reads the
//! widgets, calls the form, and shows the answer.
//!
//! The list is a `GtkTreeView` over a `GtkListStore` of two text columns.
//! No `GtkTreeIter` is ever allocated here: rows go in through
//! `gtk_list_store_insert_with_valuesv`, which accepts NULL for its
//! iterator, and the selection comes back as `GtkTreePath`s. So this crate
//! still knows the layout of no GTK struct.

use std::cell::RefCell;
use std::ffi::{CStr, c_int};
use std::ptr;
use std::sync::Arc;

use eds_sys::ESource;
use evo_sys::{
    EMailConfigPage, EMailConfigPageInterface, GTK_ORIENTATION_HORIZONTAL,
    GTK_ORIENTATION_VERTICAL, GTK_RESPONSE_CANCEL, GTK_RESPONSE_OK, GTK_SELECTION_BROWSE,
    GtkCellRenderer, GtkListStore, GtkTextBuffer, GtkTreeModel, GtkTreePath, GtkTreeSelection,
    GtkWidget, GtkWindow, e_mail_config_page_get_type, gtk_box_new, gtk_box_pack_start,
    gtk_button_new_with_mnemonic, gtk_cell_renderer_text_new, gtk_container_add,
    gtk_dialog_add_button, gtk_dialog_get_content_area, gtk_dialog_new, gtk_dialog_run,
    gtk_entry_new, gtk_label_new, gtk_label_new_with_mnemonic, gtk_label_set_mnemonic_widget,
    gtk_label_set_text, gtk_label_set_xalign, gtk_list_store_clear,
    gtk_list_store_insert_with_valuesv, gtk_list_store_newv, gtk_scrolled_window_get_type,
    gtk_text_view_get_buffer, gtk_text_view_new, gtk_text_view_set_editable,
    gtk_text_view_set_monospace, gtk_tree_path_free, gtk_tree_path_get_indices_with_depth,
    gtk_tree_path_new_from_indicesv, gtk_tree_selection_get_selected_rows,
    gtk_tree_selection_set_mode, gtk_tree_view_append_column, gtk_tree_view_column_add_attribute,
    gtk_tree_view_column_new, gtk_tree_view_column_pack_start, gtk_tree_view_column_set_expand,
    gtk_tree_view_column_set_title, gtk_tree_view_get_selection, gtk_tree_view_new_with_model,
    gtk_tree_view_set_cursor, gtk_tree_view_set_headers_visible, gtk_widget_destroy,
    gtk_widget_get_toplevel, gtk_widget_set_hexpand, gtk_widget_set_sensitive,
    gtk_widget_set_size_request, gtk_widget_set_vexpand, gtk_widget_show_all, gtk_window_get_type,
    gtk_window_set_title, gtk_window_set_transient_for,
};
use gio_sys::{
    GAsyncReadyCallback, GAsyncResult, GCancellable, GTask, g_io_error_quark, g_task_new,
    g_task_propagate_boolean, g_task_return_boolean, g_task_return_error, g_task_run_in_thread,
    g_task_set_task_data,
};
use glib_sys::{
    GError, GFALSE, GTRUE, GType, g_error_new_literal, g_list_free_full, gboolean, gpointer,
};
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

use crate::dispatch;
use crate::filters::form::{FiltersForm, NO_SCRIPT_SELECTED, ScriptEntry};
use crate::filters::io::{self, Loaded, SavePlan};
use crate::link::{self, AccountLink, SendSource};
use crate::widget::{flag, get_text, set_text};

const NAME: &CStr = c"JmapFiltersPage";
const STATE: ObjectData<RefCell<UiState>> = ObjectData::new(c"jmap-filters-page-state");

/// The columns of the list store: the script name, and whether the account
/// runs it.
const COLUMN_NAME: c_int = 0;
const COLUMN_ACTIVE: c_int = 1;

const CONTACTING: &CStr = N_(c"Contacting the mail server…");
const NOT_OFFERED: &CStr = N_(c"Your server does not offer server-side filters.");
// TRANSLATORS: %1$s is the reason the mail server gave.
const LOAD_FAILED: &CStr = N_(c"The filters could not be read: %1$s");
// TRANSLATORS: the Active column of the script list, for the one script
// the account runs. Every other row leaves the column empty.
const ACTIVE_MARK: &CStr = N_(c"Yes");
// TRANSLATORS: %1$s is the name of a filter script on the server.
const VACATION_NOTE: &CStr = N_(c"%1$s (used by the Vacation Responder)");
const ADD_TITLE: &CStr = N_(c"New Filter Script");
const RENAME_TITLE: &CStr = N_(c"Rename Filter Script");
const NAME_LABEL: &CStr = N_(c"_Name:");
const CHECKING: &CStr = N_(c"Checking the script…");
const SCRIPT_VALID: &CStr = N_(c"The script is valid.");

/// All page state, in a box as qdata. It holds the widget pointers (main
/// thread only), the staged scripts, and the account link from the load.
/// The `Arc` shares the link with the `GTask` submit threads.
struct UiState {
    link: Option<Arc<AccountLink>>,
    form: FiltersForm,
    /// The row whose source the text view currently holds.
    showing: Option<usize>,
    /// True while this page rewrites the list store. The selection handler
    /// ignores the signals that rewrite emits.
    syncing: bool,
    store: *mut GtkListStore,
    view: *mut GtkWidget,
    source_view: *mut GtkWidget,
    buffer: *mut GtkTextBuffer,
    status: *mut GtkWidget,
    add: *mut GtkWidget,
    rename: *mut GtkWidget,
    remove: *mut GtkWidget,
    activate: *mut GtkWidget,
    deactivate: *mut GtkWidget,
    check: *mut GtkWidget,
}

/// The qdata of the page, while the page is alive.
///
/// # Safety
///
/// `page` must be a live `JmapFiltersPage`. The reference must not live
/// longer than the page (callers use it in one main-loop callback).
unsafe fn state<'a>(page: *mut GObject) -> Option<&'a RefCell<UiState>> {
    // SAFETY: per the contract. instance_init sets the data one time.
    unsafe { STATE.get(page) }
}

pub struct FiltersPage;

// SAFETY: the parent is GtkScrolledWindow. type_sizes() gets its sizes from
// the running type system. The subclass adds no inline fields (the state is
// qdata). GTypeInstance and GObjectClass are prefixes of all GObject-derived
// structs, so the casts in the trampolines stay in bounds.
unsafe impl ObjectSubclass for FiltersPage {
    const NAME: &'static CStr = NAME;
    type Instance = GTypeInstance;
    type Class = GObjectClass;

    fn parent_type() -> GType {
        // `EMailConfigPage` requires GtkScrolledWindow, as the vacation page
        // explains.
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

// SAFETY: as in the vacation page. `EMailConfigPageInterface` is the bindgen
// binding of the interface struct of `e_mail_config_page_get_type`, and it
// starts with `GTypeInterface`.
unsafe impl InterfaceImpl for PageInterface {
    type Vtable = EMailConfigPageInterface;

    fn gtype() -> GType {
        // SAFETY: no arguments, and the type initializes itself.
        unsafe { e_mail_config_page_get_type() }
    }

    unsafe fn interface_init(vtable: *mut Self::Vtable) {
        // SAFETY: this is the page copy of the vtable. It contains the
        // interface defaults, so check_complete and commit_changes keep the
        // upstream defaults.
        let vtable = unsafe { &mut *vtable };
        vtable.title = translate_static(N_(c"Server Filters"));
        // Directly after the vacation page (550), the other page this
        // project adds.
        vtable.sort_order = 551;
        vtable.submit = Some(submit);
        vtable.submit_finish = Some(submit_finish);
    }
}

/// Builds the only child of the page and stores the [`UiState`]. Every
/// control starts insensitive; the load makes the usable ones sensitive.
///
/// # Safety
///
/// `page` must be a freshly initialised `JmapFiltersPage` instance.
unsafe fn build_widgets(page: *mut GObject) {
    // SAFETY: the instance is a GtkScrolledWindow that the parent
    // initialized, as the container cast requires. Each widget below is new,
    // and its container owns it.
    unsafe {
        let content = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        gtk_container_add(page.cast(), content);

        let status = gtk_label_new(ptr::null());
        gtk_label_set_xalign(status.cast(), 0.0);
        gtk_box_pack_start(content.cast(), status, GFALSE, GFALSE, 0);
        let contacting = cstring_lossy(&translate(CONTACTING));
        gtk_label_set_text(status.cast(), contacting.as_ptr());

        // The script list, beside a column of buttons that act on it.
        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        gtk_box_pack_start(content.cast(), row, GFALSE, GFALSE, 0);

        let mut types = [G_TYPE_STRING, G_TYPE_STRING];
        let store = gtk_list_store_newv(2, types.as_mut_ptr());
        let view = gtk_tree_view_new_with_model(store.cast::<GtkTreeModel>());
        // A GtkListStore is a plain GObject, not a GInitiallyUnowned, so
        // this reference is owned and not floating. The view took its own
        // reference above, so release this one: the view then owns the
        // store, and the store lives exactly as long as the view does. The
        // pointer in `UiState` stays valid for that time. The column and
        // the renderer below need no such release, because both are
        // GInitiallyUnowned and the calls that take them sink them.
        g_object_unref(store.cast());
        gtk_tree_view_set_headers_visible(view.cast(), GTRUE);
        add_text_column(view, N_(c"Name"), COLUMN_NAME, true);
        add_text_column(view, N_(c"Active"), COLUMN_ACTIVE, false);
        // The page's own scrolled window scrolls the list; it needs a
        // height of its own so that it does not collapse to one row.
        gtk_widget_set_size_request(view, -1, 160);
        gtk_widget_set_hexpand(view, GTRUE);
        gtk_box_pack_start(row.cast(), view, GTRUE, GTRUE, 0);

        let selection = gtk_tree_view_get_selection(view.cast());
        gtk_tree_selection_set_mode(selection, GTK_SELECTION_BROWSE);

        let buttons = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        gtk_box_pack_start(row.cast(), buttons, GFALSE, GFALSE, 0);
        let add = add_button(page, buttons, N_(c"_Add"), on_add);
        let rename = add_button(page, buttons, N_(c"_Rename"), on_rename);
        let remove = add_button(page, buttons, N_(c"Re_move"), on_remove);
        let activate = add_button(page, buttons, N_(c"A_ctivate"), on_activate);
        let deactivate = add_button(page, buttons, N_(c"_Deactivate"), on_deactivate);

        // The text-mode editor of decision D11: monospace, no highlighting.
        let source_view = gtk_text_view_new();
        gtk_text_view_set_monospace(source_view.cast(), GTRUE);
        gtk_widget_set_vexpand(source_view, GTRUE);
        let label = gtk_label_new_with_mnemonic(translate_static(N_(c"_Script:")));
        gtk_label_set_xalign(label.cast(), 0.0);
        gtk_label_set_mnemonic_widget(label.cast(), source_view);
        gtk_box_pack_start(content.cast(), label, GFALSE, GFALSE, 0);
        gtk_box_pack_start(content.cast(), source_view, GTRUE, GTRUE, 0);
        let buffer = gtk_text_view_get_buffer(source_view.cast());

        let check = add_button(page, content, N_(c"C_heck"), on_check);

        for control in [
            add,
            rename,
            remove,
            activate,
            deactivate,
            check,
            source_view,
        ] {
            gtk_widget_set_sensitive(control, GFALSE);
        }
        gtk_widget_show_all(page.cast());

        g_signal_connect_data(
            selection.cast(),
            c"changed".as_ptr(),
            gcallback(on_selection_changed as unsafe extern "C" fn(*mut GObject, gpointer)),
            page.cast(),
            None,
            0,
        );

        STATE.set(
            page,
            RefCell::new(UiState {
                link: None,
                form: FiltersForm::default(),
                showing: None,
                syncing: false,
                store,
                view,
                source_view,
                buffer,
                status,
                add,
                rename,
                remove,
                activate,
                deactivate,
                check,
            }),
        );
    }
}

/// Adds one text column that reads `column` of the list store.
///
/// # Safety
///
/// `view` must be a live `GtkTreeView` with a model that has `column`.
unsafe fn add_text_column(view: *mut GtkWidget, title: &'static CStr, column: c_int, expand: bool) {
    // SAFETY: the column and the renderer are new, and the view takes the
    // column, which takes the renderer.
    unsafe {
        let renderer: *mut GtkCellRenderer = gtk_cell_renderer_text_new();
        let tree_column = gtk_tree_view_column_new();
        gtk_tree_view_column_set_title(tree_column, translate_static(title));
        gtk_tree_view_column_set_expand(tree_column, flag(expand));
        gtk_tree_view_column_pack_start(tree_column, renderer, GTRUE);
        gtk_tree_view_column_add_attribute(tree_column, renderer, c"text".as_ptr(), column);
        gtk_tree_view_append_column(view.cast(), tree_column);
    }
}

/// Adds one button to `box_` and connects `handler` to its `clicked`
/// signal. The data pointer of the connection is `page`, which GTK hands
/// back to the handler.
///
/// # Safety
///
/// `box_` must be a live `GtkBox` inside `page`, a live page. The button
/// is a descendant of the page, so the page outlives it.
unsafe fn add_button(
    page: *mut GObject,
    box_: *mut GtkWidget,
    label: &'static CStr,
    handler: unsafe extern "C" fn(*mut GObject, gpointer),
) -> *mut GtkWidget {
    // SAFETY: the button is new, and the box owns it.
    unsafe {
        let button = gtk_button_new_with_mnemonic(translate_static(label));
        gtk_box_pack_start(box_.cast(), button, GFALSE, GFALSE, 0);
        g_signal_connect_data(
            button.cast(),
            c"clicked".as_ptr(),
            gcallback(handler),
            page.cast(),
            None,
            0,
        );
        button
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
        log_critical("JmapFiltersPage: not registered; was the module loaded?");
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

    // SAFETY: the source is valid, per the contract of this function. The
    // worker owns the reference and releases it.
    let source = unsafe { SendSource::adopt(g_object_ref(connect_source.cast()).cast()) };
    let work = move || {
        // SAFETY: the reference from `create` keeps the source alive for all
        // of this closure.
        let outcome = unsafe { link::connect_account(source.as_ptr()) }.and_then(|link| {
            let loaded = io::load(&link)?;
            Ok((Arc::new(link), loaded))
        });
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

/// Fills the widgets with the load result, or shows why the load failed.
///
/// # Safety
///
/// `page` must be the live page of the load.
unsafe fn apply_outcome(page: *mut GObject, outcome: Result<(Arc<AccountLink>, Loaded), String>) {
    // SAFETY: the page is live, per the contract.
    let Some(cell) = (unsafe { state(page) }) else {
        return;
    };
    match outcome {
        Ok((link, loaded)) if link.features.sieve => {
            {
                let mut ui = cell.borrow_mut();
                ui.form = FiltersForm::from_loaded(loaded);
                ui.link = Some(link);
            }
            // SAFETY: the widgets are live children of the page, and no
            // borrow is held across the GTK calls inside.
            unsafe {
                set_status(cell, "");
                refresh(cell);
            }
        }
        Ok(_) => {
            // SAFETY: as above.
            unsafe { set_status(cell, &translate(NOT_OFFERED)) };
        }
        Err(message) => {
            let text = translate_with(LOAD_FAILED, &[&message]);
            // SAFETY: as above.
            unsafe { set_status(cell, &text) };
        }
    }
}

/// Rebuilds the list from the form, then shows the selected script.
///
/// # Safety
///
/// `cell` must be the state of a live page, on the main thread. No borrow
/// of it may be held.
unsafe fn refresh(cell: &RefCell<UiState>) {
    let (store, view, rows, selected) = {
        let ui = cell.borrow();
        let rows: Vec<(String, bool)> = ui
            .form
            .entries()
            .iter()
            .map(|entry| (row_label(entry), entry.active))
            .collect();
        (ui.store, ui.view, rows, ui.form.selected())
    };

    // Rewriting the store moves the cursor, which emits "changed" on the
    // selection. That handler would read the widgets back into the form in
    // the middle of this rebuild, so it stands down while this flag is set.
    cell.borrow_mut().syncing = true;
    // SAFETY: the store and the view are live children of the page, and no
    // borrow of `cell` is held here.
    unsafe {
        gtk_list_store_clear(store);
        for (name, active) in &rows {
            let active = if *active {
                translate(ACTIVE_MARK)
            } else {
                String::new()
            };
            append_row(store, name, &active);
        }
        if let Some(row) = selected {
            set_cursor(view, row);
        }
    }
    cell.borrow_mut().syncing = false;

    // SAFETY: as above.
    unsafe { show_selected(cell) };
}

/// Puts the selected script's source in the text view and sets the
/// sensitivity of every control.
///
/// # Safety
///
/// As [`refresh`].
unsafe fn show_selected(cell: &RefCell<UiState>) {
    let (buffer, text, buttons, controls) = {
        let mut ui = cell.borrow_mut();
        let row = ui.form.selected();
        ui.showing = row;
        let text = row
            .and_then(|row| ui.form.entries().get(row))
            .map(|entry| entry.text.clone())
            .unwrap_or_default();
        let buttons = ui.form.buttons();
        let controls = [
            ui.add,
            ui.rename,
            ui.remove,
            ui.activate,
            ui.deactivate,
            ui.check,
            ui.source_view,
        ];
        (ui.buffer, text, buttons, controls)
    };
    let sensitive = [
        buttons.add,
        buttons.rename,
        buttons.remove,
        buttons.activate,
        buttons.deactivate,
        buttons.check,
        buttons.view,
    ];
    let source_view = controls[6];
    // SAFETY: the buffer and the controls are live children of the page, and
    // no borrow of `cell` is held here.
    unsafe {
        set_text(buffer.cast(), &text);
        gtk_text_view_set_editable(source_view.cast(), flag(buttons.edit));
        for (control, on) in controls.iter().zip(sensitive) {
            gtk_widget_set_sensitive(*control, flag(on));
        }
    }
}

/// Copies the text view back into the script it belongs to. Every action
/// calls this first, because the page connects to no buffer signal.
///
/// # Safety
///
/// As [`refresh`].
unsafe fn flush(cell: &RefCell<UiState>) {
    let (buffer, showing) = {
        let ui = cell.borrow();
        (ui.buffer, ui.showing)
    };
    let Some(row) = showing else {
        return;
    };
    // SAFETY: the buffer is a live child of the page, and no borrow is held.
    let text = unsafe { get_text(buffer.cast()) };
    // A script the vacation responder owns refuses the write, which is the
    // wanted answer: its text view is read-only anyway.
    let _ = cell.borrow_mut().form.set_text(row, &text);
}

/// What the Name column shows: the script's name, and for the script the
/// vacation responder owns, a note that this page does not change it.
fn row_label(entry: &ScriptEntry) -> String {
    if entry.protected {
        translate_with(VACATION_NOTE, &[&entry.name])
    } else {
        entry.name.clone()
    }
}

/// Writes `text` in the status label.
///
/// # Safety
///
/// As [`refresh`].
unsafe fn set_status(cell: &RefCell<UiState>, text: &str) {
    let status = cell.borrow().status;
    let text = cstring_lossy(text);
    // SAFETY: the label is a live child of the page.
    unsafe { gtk_label_set_text(status.cast(), text.as_ptr()) };
}

/// Appends one row to the list store.
///
/// # Safety
///
/// `store` must be the live two-string-column store of a live page.
unsafe fn append_row(store: *mut GtkListStore, name: &str, active: &str) {
    let name = cstring_lossy(name);
    let active = cstring_lossy(active);
    let mut columns = [COLUMN_NAME, COLUMN_ACTIVE];
    // SAFETY: a zeroed GValue is `G_VALUE_INIT`, the state `g_value_init`
    // requires. Both values are strings, which the store's two columns hold,
    // and both are released below. NULL as the iterator means "do not report
    // the new row back", which this caller does not need.
    unsafe {
        let mut values: [GValue; 2] = [std::mem::zeroed(), std::mem::zeroed()];
        g_value_init(&mut values[0], G_TYPE_STRING);
        g_value_set_string(&mut values[0], name.as_ptr());
        g_value_init(&mut values[1], G_TYPE_STRING);
        g_value_set_string(&mut values[1], active.as_ptr());
        gtk_list_store_insert_with_valuesv(
            store,
            ptr::null_mut(),
            -1,
            columns.as_mut_ptr(),
            values.as_mut_ptr(),
            2,
        );
        g_value_unset(&mut values[0]);
        g_value_unset(&mut values[1]);
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

/// Moves the cursor of the list to `row`.
///
/// # Safety
///
/// `view` must be the live list of a page, and `row` must exist in it.
unsafe fn set_cursor(view: *mut GtkWidget, row: usize) {
    let Ok(row) = c_int::try_from(row) else {
        return;
    };
    let mut indices = [row];
    // SAFETY: a one-level path for a list store. The path is new, and this
    // function frees it.
    unsafe {
        let path = gtk_tree_path_new_from_indicesv(indices.as_mut_ptr(), 1);
        if path.is_null() {
            return;
        }
        gtk_tree_view_set_cursor(view.cast(), path, ptr::null_mut(), GFALSE);
        gtk_tree_path_free(path);
    }
}

/// The `changed` handler of the list selection.
///
/// # Safety
///
/// The GLib signal system calls this. `selection` is the selection of the
/// page's list, and `page` is the data pointer of the connection.
unsafe extern "C" fn on_selection_changed(selection: *mut GObject, page: gpointer) {
    guard("JmapFiltersPage::selection-changed", (), || {
        // SAFETY: the page is live, because its descendant emitted.
        let Some(cell) = (unsafe { state(page.cast()) }) else {
            return;
        };
        if cell.borrow().syncing {
            return;
        }
        // SAFETY: the widgets are live, and no borrow is held across these.
        unsafe {
            flush(cell);
            let row = selected_row(selection.cast());
            cell.borrow_mut().form.select(row);
            show_selected(cell);
        }
    });
}

/// What one button press asks the form to do.
enum Action {
    Add,
    Rename,
    Remove,
    Activate,
    Deactivate,
}

unsafe extern "C" fn on_add(_button: *mut GObject, page: gpointer) {
    // SAFETY: the page is live, because its button emitted.
    unsafe { act(page, Action::Add) };
}

unsafe extern "C" fn on_rename(_button: *mut GObject, page: gpointer) {
    // SAFETY: as above.
    unsafe { act(page, Action::Rename) };
}

unsafe extern "C" fn on_remove(_button: *mut GObject, page: gpointer) {
    // SAFETY: as above.
    unsafe { act(page, Action::Remove) };
}

unsafe extern "C" fn on_activate(_button: *mut GObject, page: gpointer) {
    // SAFETY: as above.
    unsafe { act(page, Action::Activate) };
}

unsafe extern "C" fn on_deactivate(_button: *mut GObject, page: gpointer) {
    // SAFETY: as above.
    unsafe { act(page, Action::Deactivate) };
}

/// Runs one staged action and shows the list again. A refusal goes to the
/// status label; nothing reaches the server here.
///
/// # Safety
///
/// `page` must be the live page the button belongs to.
unsafe fn act(page: gpointer, action: Action) {
    guard("JmapFiltersPage::action", (), || {
        // SAFETY: the page is live, per the contract.
        let Some(cell) = (unsafe { state(page.cast()) }) else {
            return;
        };
        // SAFETY: the widgets are live, and no borrow is held.
        unsafe { flush(cell) };
        let selected = cell.borrow().form.selected();

        // The name dialog runs a nested main loop, so it must run with no
        // borrow of the state held: the handlers it lets through take their
        // own.
        let changed = match action {
            Action::Add => {
                // SAFETY: the page is live, per the contract.
                match unsafe { ask_for_name(page.cast(), ADD_TITLE, "") } {
                    None => Ok(false),
                    Some(name) => cell.borrow_mut().form.add(&name).map(|_| true),
                }
            }
            Action::Rename => {
                let Some(row) = selected else {
                    return refuse(cell, NO_SCRIPT_SELECTED);
                };
                let current = cell
                    .borrow()
                    .form
                    .entries()
                    .get(row)
                    .map(|entry| entry.name.clone())
                    .unwrap_or_default();
                // SAFETY: as above.
                match unsafe { ask_for_name(page.cast(), RENAME_TITLE, &current) } {
                    None => Ok(false),
                    Some(name) => cell.borrow_mut().form.rename(row, &name).map(|()| true),
                }
            }
            Action::Remove => match selected {
                None => return refuse(cell, NO_SCRIPT_SELECTED),
                Some(row) => cell.borrow_mut().form.remove(row).map(|()| true),
            },
            Action::Activate => match selected {
                None => return refuse(cell, NO_SCRIPT_SELECTED),
                Some(row) => cell.borrow_mut().form.activate(row).map(|()| true),
            },
            Action::Deactivate => match selected {
                None => return refuse(cell, NO_SCRIPT_SELECTED),
                Some(row) => cell.borrow_mut().form.deactivate(row).map(|()| true),
            },
        };

        match changed {
            Ok(false) => {}
            Ok(true) => {
                // SAFETY: the widgets are live, and no borrow is held.
                unsafe {
                    set_status(cell, "");
                    refresh(cell);
                }
            }
            Err(reason) => refuse(cell, reason),
        }
    });
}

/// Shows why the page did not do what the button asked.
fn refuse(cell: &RefCell<UiState>, reason: &CStr) {
    // SAFETY: the status label is a live child of the page, and no borrow is
    // held across the call.
    unsafe { set_status(cell, &translate(reason)) };
}

/// Asks for a script name in a modal dialog. `None` means the user
/// cancelled.
///
/// # Safety
///
/// `page` must be the live page, on the main thread.
unsafe fn ask_for_name(page: *mut GObject, title: &'static CStr, initial: &str) -> Option<String> {
    // SAFETY: the page is live, per the contract. Every widget is new, the
    // dialog owns it, and this function destroys the dialog.
    unsafe {
        let dialog = gtk_dialog_new();
        gtk_window_set_title(dialog.cast(), translate_static(title));
        let toplevel = gtk_widget_get_toplevel(page.cast());
        // Before the editor packs the page, its toplevel is the page itself,
        // which is no window. Then the dialog has no parent, which GTK
        // allows.
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
        gtk_dialog_add_button(dialog.cast(), translate_static(N_(c"_OK")), GTK_RESPONSE_OK);

        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        let label = gtk_label_new_with_mnemonic(translate_static(NAME_LABEL));
        let entry = gtk_entry_new();
        set_text(entry.cast(), initial);
        gtk_label_set_mnemonic_widget(label.cast(), entry);
        gtk_box_pack_start(row.cast(), label, GFALSE, GFALSE, 0);
        gtk_widget_set_hexpand(entry, GTRUE);
        gtk_box_pack_start(row.cast(), entry, GTRUE, GTRUE, 0);
        gtk_container_add(gtk_dialog_get_content_area(dialog.cast()).cast(), row);
        gtk_widget_show_all(dialog);

        let response = gtk_dialog_run(dialog.cast());
        let typed = (response == GTK_RESPONSE_OK).then(|| get_text(entry.cast()));
        gtk_widget_destroy(dialog);
        typed
    }
}

/// The data one "Check" needs on its worker thread.
struct CheckJob {
    link: Arc<AccountLink>,
    text: String,
}

/// The `clicked` handler of the Check button. It is the one action that
/// reaches the server before OK, and it writes nothing (decision D4).
///
/// # Safety
///
/// The GLib signal system calls this with the page as the data pointer.
unsafe extern "C" fn on_check(_button: *mut GObject, page: gpointer) {
    guard("JmapFiltersPage::check", (), || {
        // SAFETY: the page is live, because its button emitted.
        let Some(cell) = (unsafe { state(page.cast()) }) else {
            return;
        };
        // SAFETY: the widgets are live, and no borrow is held.
        unsafe { flush(cell) };
        let job = {
            let ui = cell.borrow();
            let link = ui.link.clone();
            let text = ui
                .form
                .selected()
                .and_then(|row| ui.form.entries().get(row))
                .map(|entry| entry.text.clone());
            link.zip(text).map(|(link, text)| CheckJob { link, text })
        };
        let Some(job) = job else {
            return refuse(cell, NO_SCRIPT_SELECTED);
        };
        // SAFETY: the widgets are live, and no borrow is held.
        unsafe { set_status(cell, &translate(CHECKING)) };

        let work = move || io::check(&job.link, &job.text);
        let finish = |page: *mut GObject, answer: Result<Option<String>, String>| {
            // SAFETY: `with_strong` gives the live page.
            let Some(cell) = (unsafe { state(page) }) else {
                return;
            };
            let text = match answer {
                Ok(None) => translate(SCRIPT_VALID),
                Ok(Some(detail)) | Err(detail) => detail,
            };
            // SAFETY: as above.
            unsafe { set_status(cell, &text) };
        };
        // SAFETY: the page is live and the signal emission holds a
        // reference to it for all of this call.
        unsafe { dispatch::spawn_for(page.cast(), work, finish) };
    });
}

/// The data that one submit writes, for the `GTask` thread. The link keeps
/// the client alive. The plan is already checked against the server limits.
struct SubmitJob {
    link: Arc<AccountLink>,
    plan: SavePlan,
}

unsafe extern "C" fn drop_job(data: gpointer) {
    // SAFETY: this frees the box that `submit` leaked. The task calls this
    // one time, when it drops its task data.
    drop(unsafe { Box::from_raw(data.cast::<SubmitJob>()) });
}

/// The blocking part, on the worker thread of the task.
unsafe extern "C" fn submit_thread(
    task: *mut GTask,
    _source_object: *mut GObject,
    task_data: gpointer,
    _cancellable: *mut GCancellable,
) {
    guard("JmapFiltersPage::submit_thread", (), || {
        // SAFETY: `submit` attached the job. This is a borrow; the task frees it.
        let job = unsafe { &*task_data.cast::<SubmitJob>() };
        match io::save(&job.link, &job.plan) {
            // SAFETY: `task` is the live task of this thread function.
            Ok(()) => unsafe { g_task_return_boolean(task, GTRUE) },
            Err(message) => {
                let text = cstring_lossy(&message);
                // SAFETY: a new GError; the task gets its ownership.
                unsafe {
                    let error = g_error_new_literal(g_io_error_quark(), 0, text.as_ptr());
                    g_task_return_error(task, error);
                }
            }
        }
    });
}

/// `EMailConfigPage.submit`: the save button of the account editor.
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
    guard("JmapFiltersPage::submit", (), || {
        // SAFETY: the page is live, per the vfunc contract. This function
        // releases the task after hand-off (GTask stays alive until it returns).
        let task = unsafe { g_task_new(page.cast(), cancellable, callback, user_data) };

        // SAFETY: the page is live, and the text view is its live child.
        let planned = unsafe { state(page.cast()) }.map(|cell| {
            unsafe { flush(cell) };
            let ui = cell.borrow();
            (ui.link.clone(), ui.form.plan())
        });

        match planned {
            // No link, no plan, or nothing changed: Evolution submits every
            // page on every save, and a page without changes must not write.
            None | Some((None, _)) | Some((_, Ok(None))) => {
                // SAFETY: completes the new task and releases the local reference.
                unsafe {
                    g_task_return_boolean(task, GTRUE);
                    g_object_unref(task.cast());
                }
            }
            Some((_, Err(reason))) => {
                let text = cstring_lossy(&reason);
                // SAFETY: as above. The task gets the ownership of the error.
                unsafe {
                    let error = g_error_new_literal(g_io_error_quark(), 0, text.as_ptr());
                    g_task_return_error(task, error);
                    g_object_unref(task.cast());
                }
            }
            Some((Some(link), Ok(Some(plan)))) => {
                let job = Box::new(SubmitJob { link, plan });
                // SAFETY: the task is live and gets the ownership of the
                // data (drop_job). run_in_thread keeps its own reference,
                // so this releases the local reference.
                unsafe {
                    g_task_set_task_data(task, Box::into_raw(job).cast(), Some(drop_job));
                    g_task_run_in_thread(task, Some(submit_thread));
                    g_object_unref(task.cast());
                }
            }
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
    error: *mut *mut GError,
) -> gboolean {
    guard("JmapFiltersPage::submit_finish", GFALSE, || {
        // SAFETY: `result` is the task from submit. The GIO contract gives
        // finish the same result that the callback received.
        unsafe { g_task_propagate_boolean(result.cast::<GTask>(), error) }
    })
}
