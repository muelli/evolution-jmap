// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapFiltersPage`: the Server Filters page of the account editor. It is a
//! `GtkScrolledWindow` subclass that implements `EMailConfigPage`, as
//! [`crate::vacation::page`] is, and it follows that page in every structural
//! decision. Read that file first: it explains the parent choice, the qdata
//! state box, the asynchronous load and the `GTask` submit.
//!
//! This page shows the account's Sieve scripts as a list, and the source of
//! the selected one in a text view. Every change stages in a [`FiltersForm`]
//! first; item 180(a)'s apply-at-once sweep (one action per commit) then
//! writes most of them at once instead of waiting for the account editor's
//! save: "Check" asks the server and writes nothing; "Activate",
//! "Deactivate", "Rename", "Add" and "Remove" write the account's current
//! plan and reload the list from the server; a script's text saves the same
//! way once it parses, after the debounce that also drives [`validate`] and
//! completion (`maybe_save_text`).
//!
//! The widgets hold no decisions. The form answers which buttons are usable
//! and what one submit writes, and it has unit tests. This file reads the
//! widgets, calls the form, and shows the answer.
//!
//! The list is a `GtkTreeView` over a `GtkListStore` of two text columns.
//! No `GtkTreeIter` is ever allocated here: rows go in through
//! `gtk_list_store_insert_with_valuesv`, which accepts NULL for its
//! iterator, and the selection comes back as `GtkTreePath`s.
//!
//! The text-mode editor's error underline (item 171b) is the one exception:
//! it allocates a `GtkTextIter` to find and tag the line a local Sieve parse
//! rejects. `evo-sys` gives that one struct a real layout instead of a
//! zero-sized handle; see the comment on its `build.rs` definition.

use std::cell::RefCell;
use std::ffi::{CStr, c_int, c_uint, c_ulong};
use std::path::Path;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use eds_sys::{ESource, e_source_get_uid};
use evo_sys::{
    EMailConfigPage, EMailConfigPageInterface, GDK_CONTROL_MASK, GDK_KEY_Down, GDK_KEY_Escape,
    GDK_KEY_Return, GDK_KEY_Right, GDK_KEY_Tab, GDK_KEY_Up, GDK_KEY_space,
    GTK_ORIENTATION_HORIZONTAL, GTK_ORIENTATION_VERTICAL, GTK_RESPONSE_CANCEL, GTK_RESPONSE_OK,
    GTK_SELECTION_BROWSE, GTK_TEXT_WINDOW_WIDGET, GTK_WINDOW_POPUP, GdkEventKey, GdkRectangle,
    GtkCellRenderer, GtkListStore, GtkSourceBuffer, GtkTextBuffer, GtkTextIter, GtkTextTag,
    GtkTreeModel, GtkTreePath, GtkTreeSelection, GtkWidget, GtkWindow, e_mail_config_page_get_type,
    e_shell_get_default, gdk_event_get_keyval, gdk_event_get_state, gdk_window_get_origin,
    gtk_box_new, gtk_box_pack_start, gtk_button_new_with_mnemonic, gtk_cell_renderer_text_new,
    gtk_check_button_new_with_mnemonic, gtk_container_add, gtk_container_get_children,
    gtk_container_set_border_width, gtk_dialog_add_button, gtk_dialog_get_content_area,
    gtk_dialog_new, gtk_dialog_run, gtk_dialog_set_default_response, gtk_entry_new,
    gtk_entry_set_activates_default, gtk_label_new, gtk_label_new_with_mnemonic,
    gtk_label_set_mnemonic_widget, gtk_label_set_text, gtk_label_set_xalign, gtk_list_store_clear,
    gtk_list_store_insert_with_valuesv, gtk_list_store_newv, gtk_scrolled_window_get_type,
    gtk_source_buffer_set_language, gtk_source_language_manager_get_default,
    gtk_source_language_manager_get_language, gtk_source_view_new, gtk_source_view_set_auto_indent,
    gtk_text_buffer_apply_tag, gtk_text_buffer_delete, gtk_text_buffer_get_end_iter,
    gtk_text_buffer_get_insert, gtk_text_buffer_get_iter_at_line, gtk_text_buffer_get_iter_at_mark,
    gtk_text_buffer_get_start_iter, gtk_text_buffer_get_tag_table, gtk_text_buffer_get_text,
    gtk_text_buffer_insert, gtk_text_buffer_place_cursor, gtk_text_buffer_remove_tag,
    gtk_text_iter_backward_chars, gtk_text_iter_forward_to_line_end, gtk_text_iter_get_line,
    gtk_text_tag_new, gtk_text_tag_table_add, gtk_text_view_buffer_to_window_coords,
    gtk_text_view_get_buffer, gtk_text_view_get_iter_location, gtk_text_view_set_accepts_tab,
    gtk_text_view_set_editable, gtk_text_view_set_monospace, gtk_tree_path_free,
    gtk_tree_path_get_indices_with_depth, gtk_tree_path_new_from_indicesv,
    gtk_tree_selection_get_selected_rows, gtk_tree_selection_set_mode, gtk_tree_view_append_column,
    gtk_tree_view_column_add_attribute, gtk_tree_view_column_new, gtk_tree_view_column_pack_start,
    gtk_tree_view_column_set_expand, gtk_tree_view_column_set_title, gtk_tree_view_get_selection,
    gtk_tree_view_new_with_model, gtk_tree_view_set_cursor, gtk_tree_view_set_headers_visible,
    gtk_widget_destroy, gtk_widget_get_toplevel, gtk_widget_get_window, gtk_widget_hide,
    gtk_widget_set_hexpand, gtk_widget_set_sensitive, gtk_widget_set_size_request,
    gtk_widget_set_tooltip_text, gtk_widget_set_vexpand, gtk_widget_set_visible,
    gtk_widget_show_all, gtk_window_get_type, gtk_window_move, gtk_window_new,
    gtk_window_set_accept_focus, gtk_window_set_title, gtk_window_set_transient_for,
};
use gio_sys::{
    GAsyncReadyCallback, GAsyncResult, GCancellable, GTask, g_io_error_quark, g_task_new,
    g_task_propagate_boolean, g_task_return_boolean, g_task_return_error, g_task_run_in_thread,
    g_task_set_task_data,
};
use glib_sys::{
    G_SOURCE_REMOVE, GError, GFALSE, GTRUE, GType, g_error_new_literal, g_list_free,
    g_list_free_full, g_source_remove, g_timeout_add, gboolean, gpointer,
};
use gobject_sys::{
    G_TYPE_STRING, GObject, GObjectClass, GTypeInstance, GValue, g_object_ref, g_object_unref,
    g_signal_connect_data, g_signal_handler_disconnect, g_type_check_instance_is_a,
    g_type_from_name, g_value_init, g_value_set_string, g_value_unset,
};
use jmap_backend_core::callback::gcallback;
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::i18n::{N_, translate, translate_static, translate_with};
use jmap_backend_core::marshal::{read_string, take_string};
use jmap_backend_core::object_data::ObjectData;
use jmap_backend_core::subclass::{InterfaceDecl, InterfaceImpl, ObjectSubclass, TypeSizes};
use jmap_backend_core::trampoline::{guard, log_critical};

use crate::dispatch;
use crate::filters::cache;
use crate::filters::completion;
use crate::filters::form::{Buttons, FiltersForm, NO_SCRIPT_SELECTED, ScriptEntry};
use crate::filters::io::{self, ActivateOutcome, Loaded, SavePlan};
use crate::filters::rule_dialog;
use crate::filters::rule_form::{self, NO_RULE_SELECTED, RuleDraft, RulesEditor};
use crate::filters::rules::MANAGED_SCRIPT_NAME;
use crate::filters::script;
use crate::filters::validate;
use crate::hold::Hold;
use crate::link::{self, AccountLink, LoadOutcome, SendSource};
use crate::widget::{self, flag, get_bool, get_text, set_bool, set_text};

const NAME: &CStr = c"JmapFiltersPage";
const STATE: ObjectData<RefCell<UiState>> = ObjectData::new(c"jmap-filters-page-state");

/// The columns of the list store: the script name, and whether the account
/// runs it.
const COLUMN_NAME: c_int = 0;
const COLUMN_ACTIVE: c_int = 1;

/// GTK's own `wrap-mode` value with the same meaning as
/// [`crate::widget::PANGO_WRAP_WORD_CHAR`], for the script text view
/// (`gtkenums.h`: NONE = 0, CHAR = 1, WORD = 2, WORD_CHAR = 3; a different
/// enum from Pango's, with different values).
const GTK_WRAP_WORD_CHAR: c_int = 3;
/// `GtkScrolledWindow`'s `hscrollbar-policy` value that keeps the page from
/// ever scrolling sideways; a wrapped label or script line gives up height
/// instead (`gtkscrolledwindow.h`: ALWAYS = 0, AUTOMATIC = 1, NEVER = 2).
const GTK_POLICY_NEVER: c_int = 2;
/// `GtkTreeViewColumn`'s `sizing` value that keeps a column's width from
/// growing with its content, so a long script or rule name cannot force the
/// page wider than the window (`gtktreeviewcolumn.h`: GROW_ONLY = 0,
/// AUTOSIZE = 1, FIXED = 2).
const GTK_TREE_VIEW_COLUMN_FIXED: c_int = 2;
/// A starting width for a fixed-sizing name column; `expand` on the column
/// still lets it grow into whatever room the view actually has.
const NAME_COLUMN_WIDTH: c_int = 160;
/// How long the text view waits after the last keystroke before running the
/// local Sieve parse (item 171b). `SieveScript/validate` is a server round
/// trip and is out of scope here; this only bounds how often the parser
/// this crate runs locally reparses the whole script.
const VALIDATE_DEBOUNCE_MS: c_uint = 300;
/// How many matches the completion popup (item 171c) shows at once. A
/// short, fixed list rather than a scrollable one: this slice's own later
/// follow-up, not this one.
const MAX_COMPLETION_ROWS: usize = 15;

const CONTACTING: &CStr = N_(c"Contacting the mail server…");
const NOT_OFFERED: &CStr = N_(c"Your server does not offer server-side filters.");
const OFFLINE: &CStr =
    N_(c"You are offline. Filters cannot be read or changed until you reconnect.");
// TRANSLATORS: %1$s is the reason the mail server gave.
const LOAD_FAILED: &CStr = N_(c"The filters could not be read: %1$s");
// TRANSLATORS: the Active column of the script list, for the one script
// the account runs. Every other row leaves the column empty.
const ACTIVE_MARK: &CStr = N_(c"Yes");
// TRANSLATORS: %1$s is the name of a filter script on the server.
const VACATION_NOTE: &CStr = N_(c"%1$s (used by the Vacation Responder)");
const ADD_TITLE: &CStr = N_(c"New Filter Script");
const RENAME_TITLE: &CStr = N_(c"Rename Filter Script");
// GNOME HIG: a button that opens a dialog gets a trailing ellipsis.
const ADD_LABEL: &CStr = N_(c"_Add…");
const RENAME_LABEL: &CStr = N_(c"_Rename…");
const NAME_LABEL: &CStr = N_(c"_Name:");
const CHECKING: &CStr = N_(c"Checking the script…");
const SCRIPT_VALID: &CStr = N_(c"The script is valid.");
const ACTIVATING: &CStr = N_(c"Activating…");
// TRANSLATORS: %1$s is the reason the mail server gave.
const ACTIVATE_FAILED: &CStr = N_(c"The script could not be activated: %1$s");
// TRANSLATORS: %1$s is the reason the mail server gave. The activation
// already took effect; only the list on this page could not be refreshed.
const ACTIVATE_RELOAD_FAILED: &CStr = N_(c"Activated. The list could not be refreshed: %1$s");
const DEACTIVATING: &CStr = N_(c"Deactivating…");
// TRANSLATORS: %1$s is the reason the mail server gave.
const DEACTIVATE_FAILED: &CStr = N_(c"The script could not be deactivated: %1$s");
// TRANSLATORS: %1$s is the reason the mail server gave. The deactivation
// already took effect; only the list on this page could not be refreshed.
const DEACTIVATE_RELOAD_FAILED: &CStr = N_(c"Deactivated. The list could not be refreshed: %1$s");
const RENAMING: &CStr = N_(c"Renaming…");
// TRANSLATORS: %1$s is the reason the mail server gave.
const RENAME_FAILED: &CStr = N_(c"The script could not be renamed: %1$s");
// TRANSLATORS: %1$s is the reason the mail server gave. The rename already
// took effect; only the list on this page could not be refreshed.
const RENAME_RELOAD_FAILED: &CStr = N_(c"Renamed. The list could not be refreshed: %1$s");
const ADDING: &CStr = N_(c"Adding…");
// TRANSLATORS: %1$s is the reason the mail server gave.
const ADD_FAILED: &CStr = N_(c"The script could not be added: %1$s");
// TRANSLATORS: %1$s is the reason the mail server gave. The script was
// already created; only the list on this page could not be refreshed.
const ADD_RELOAD_FAILED: &CStr = N_(c"Added. The list could not be refreshed: %1$s");
const REMOVING: &CStr = N_(c"Removing…");
// TRANSLATORS: %1$s is the reason the mail server gave.
const REMOVE_FAILED: &CStr = N_(c"The script could not be removed: %1$s");
// TRANSLATORS: %1$s is the reason the mail server gave. The script was
// already removed; only the list on this page could not be refreshed.
const REMOVE_RELOAD_FAILED: &CStr = N_(c"Removed. The list could not be refreshed: %1$s");
const SAVING_TEXT: &CStr = N_(c"Saving…");
// TRANSLATORS: %1$s is the reason the mail server gave. The text stays in
// the editor either way.
const SAVE_TEXT_FAILED: &CStr = N_(c"The script could not be saved: %1$s");
// TRANSLATORS: %1$s is the reason the mail server gave. The save already
// took effect; only the list on this page could not be refreshed.
const SAVE_TEXT_RELOAD_FAILED: &CStr = N_(c"Saved. The list could not be refreshed: %1$s");
// Neither list has a visible label of its own to name it for a screen
// reader through a `labelled-by` relation (item 174d), unlike the text
// view below, which has "_Script:".
const SCRIPT_LIST_NAME: &CStr = N_(c"Filter scripts");
const RULE_LIST_NAME: &CStr = N_(c"Rules of the selected script");

/// The columns of the rules list: whether the rule runs, and its name.
const RULE_COLUMN_ENABLED: c_int = 0;
const RULE_COLUMN_NAME: c_int = 1;

const ADD_RULE_TITLE: &CStr = N_(c"New Rule");
const EDIT_RULE_TITLE: &CStr = N_(c"Edit Rule");
const EDIT_AS_TEXT_LABEL: &CStr = N_(c"_Edit as text");
// Mnemonic on "t", not "E": "_Edit as text" is shown at the same time and
// already owns Alt+E. GNOME HIG: a button that opens a dialog gets a
// trailing ellipsis.
const RULE_EDIT_LABEL: &CStr = N_(c"Edi_t…");
// Mnemonic on "i", not "D": the rules list's "A_dd" button is shown at the
// same time and already owns Alt+D.
const DEACTIVATE_LABEL: &CStr = N_(c"Deact_ivate");
// Mnemonic on "v", not "A" or "C": the script list's "_Add…" button already
// owns Alt+A, and the Account Editor dialog's own stock "_Cancel" button is
// shown in the same window and already owns Alt+C.
const ACTIVATE_LABEL: &CStr = N_(c"Acti_vate");
// GNOME HIG: a button that opens a dialog gets a trailing ellipsis.
const RULE_ADD_LABEL: &CStr = N_(c"A_dd…");
const EXTERNAL_EDIT: &CStr = N_(c"Not in Evolution's rules format, so it shows as text.");

/// All page state, in a box as qdata. It holds the widget pointers (main
/// thread only), the staged scripts, and the account link from the load.
/// The `Arc` shares the link with the `GTask` submit threads.
struct UiState {
    link: Option<Arc<AccountLink>>,
    form: FiltersForm,
    /// The row whose source the text view or the rules list currently holds.
    showing: Option<usize>,
    /// The managed script's rules, while `showing` is it and rules mode is
    /// the one in view. `None` for every other script, and for the managed
    /// script while it fails to parse or the user forced text mode.
    rules: Option<RulesEditor>,
    /// True once the user ticks "Edit as text" for the script in `showing`.
    /// Reset whenever `showing` changes.
    force_text: bool,
    /// True while this page rewrites a list store or a toggle button. The
    /// selection and toggled handlers ignore the signals that rewrite emits.
    syncing: bool,
    store: *mut GtkListStore,
    view: *mut GtkWidget,
    source_label: *mut GtkWidget,
    source_view: *mut GtkWidget,
    buffer: *mut GtkTextBuffer,
    status: *mut GtkWidget,
    add: *mut GtkWidget,
    rename: *mut GtkWidget,
    remove: *mut GtkWidget,
    activate: *mut GtkWidget,
    deactivate: *mut GtkWidget,
    check: *mut GtkWidget,
    edit_as_text: *mut GtkWidget,
    rules_box: *mut GtkWidget,
    rules_store: *mut GtkListStore,
    rules_view: *mut GtkWidget,
    rule_add: *mut GtkWidget,
    rule_edit: *mut GtkWidget,
    rule_remove: *mut GtkWidget,
    rule_up: *mut GtkWidget,
    rule_down: *mut GtkWidget,
    rule_enable: *mut GtkWidget,
    rule_disable: *mut GtkWidget,
    /// The generation of the last Check click. A reply tagged with any other
    /// generation is stale and must not overwrite the status label.
    check_generation: u64,
    /// The tag item 171b applies to the line a local Sieve parse rejects.
    /// Created once, in [`build_widgets`], and kept for the page's whole
    /// life.
    error_tag: *mut GtkTextTag,
    /// The pending debounce timeout of the last buffer edit, or 0 if none is
    /// pending. [`Drop`] cancels it, so a page closed mid-debounce never runs
    /// the callback against a freed page.
    validate_timeout: c_uint,
    /// True while the script text's own debounce save (item 180(a)) is in
    /// flight. The debounce reschedules itself instead of starting a second
    /// write while this is set, the same way the Activate/Deactivate/Rename
    /// instant-apply writes rely on insensitive controls to block a second
    /// click.
    saving_text: bool,
    /// Item 171c's completion popup: a standalone `GTK_WINDOW_POPUP`, never
    /// a child of this page's own widget tree, so [`Drop`] destroys it
    /// explicitly. Created once, in [`build_widgets`], and kept for the
    /// page's whole life, like `error_tag`.
    completion_window: *mut GtkWidget,
    /// The one child `completion_window` holds: a vertical box of labels,
    /// rebuilt from `completion_candidates` each time the list or the
    /// selection changes.
    completion_list: *mut GtkWidget,
    /// The candidates the popup currently shows. Empty means the popup is
    /// hidden: this is the one flag this page uses to ask "is it open".
    completion_candidates: Vec<&'static completion::Candidate>,
    /// The index into `completion_candidates` Up/Down moves, and Tab/Enter
    /// accepts.
    completion_selected: usize,
    /// True once this page is read-only: set either by
    /// [`apply_offline_change`], item 180(c)'s "went offline while the page
    /// was open" half, or by [`apply_outcome`] finding a cold-start cache
    /// hit for a page whose initial load never ran at all (`link` stays
    /// `None` in that second case). A page whose initial load never ran and
    /// found no cache leaves this `false`: it shows its own offline message
    /// instead, with nothing to lock.
    offline: bool,
    /// The shell singleton [`build_widgets`] connected the two handlers
    /// below to, or null if none was running yet (a headless test, or no
    /// shell instance up). Kept only so [`Drop`] can disconnect from the
    /// same instance.
    shell: *mut GObject,
    /// The `notify::online` handler id on `shell`, or 0 if never connected.
    shell_online_handler: c_ulong,
    /// The `notify::network-available` handler id on `shell`, or 0 if never
    /// connected.
    shell_network_handler: c_ulong,
}

/// A fresh generation for one Check click, process-wide so it never repeats.
static CHECK_GENERATIONS: AtomicU64 = AtomicU64::new(0);

impl UiState {
    /// Marks a new Check in flight. Returns the generation to tag its reply
    /// with.
    fn begin_check(&mut self) -> u64 {
        self.check_generation = CHECK_GENERATIONS.fetch_add(1, Ordering::SeqCst);
        self.check_generation
    }

    /// True if `generation` is the latest Check this page started. A click
    /// started after it makes this one stale.
    fn check_is_current(&self, generation: u64) -> bool {
        self.check_generation == generation
    }
}

impl Drop for UiState {
    /// Cancels a pending debounce timeout, destroys the completion popup
    /// and disconnects from the shell singleton. Without the first, closing
    /// the account editor while a timeout is still pending would run it
    /// against a page `g_object_set_data_full` is already in the middle of
    /// freeing. Without the second, the popup would leak: it is a
    /// standalone toplevel window, not a child this page's own container
    /// destroys for it. Without the third, the shell (which outlives this
    /// page) would call back into freed memory the next time it goes
    /// online or offline (item 180(c)).
    fn drop(&mut self) {
        if self.validate_timeout != 0 {
            // SAFETY: a source id this page itself added with `g_timeout_add`
            // and has not yet seen fire (it would have cleared the field).
            unsafe { g_source_remove(self.validate_timeout) };
        }
        if !self.completion_window.is_null() {
            // SAFETY: a window this page created in `build_widgets` and
            // never shares with anything else.
            unsafe { gtk_widget_destroy(self.completion_window) };
        }
        if !self.shell.is_null() {
            // SAFETY: `shell` is the live singleton `build_widgets` connected
            // these handlers to; it outlives this page. A 0 id was never
            // connected, and `g_signal_handler_disconnect` is only called
            // for the other case.
            unsafe {
                if self.shell_online_handler != 0 {
                    g_signal_handler_disconnect(self.shell, self.shell_online_handler);
                }
                if self.shell_network_handler != 0 {
                    g_signal_handler_disconnect(self.shell, self.shell_network_handler);
                }
            }
        }
    }
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

/// Looks up this project's "sieve" language definition and tags `buffer`
/// with it, so GtkSourceView highlights the text-mode editor's content.
/// Does nothing if the language cannot be found (an install with no
/// `sieve.lang` in GtkSourceView's search path): the buffer still edits and
/// saves Sieve text correctly, just without colour, the same fallback a
/// missing `.lang` file gives any other GtkSourceView application.
///
/// # Safety
///
/// `buffer` must be a live `GtkSourceBuffer`, on the main thread.
unsafe fn set_sieve_language(buffer: *mut GtkSourceBuffer) {
    // SAFETY: the default manager is a process-wide singleton GtkSourceView
    // owns; the language it returns, if any, is owned by the manager too
    // and outlives this call.
    unsafe {
        let manager = gtk_source_language_manager_get_default();
        let language = gtk_source_language_manager_get_language(manager, c"sieve".as_ptr());
        if !language.is_null() {
            gtk_source_buffer_set_language(buffer, language);
        }
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
        // A long server error must not push the script list's own buttons
        // off screen; it wraps or grows the page's height instead.
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
        widget::set_accessible_name(view.cast(), &translate(SCRIPT_LIST_NAME));
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
        let add = add_button(page, buttons, ADD_LABEL, on_add);
        let rename = add_button(page, buttons, RENAME_LABEL, on_rename);
        let remove = add_button(page, buttons, N_(c"Re_move"), on_remove);
        let activate = add_button(page, buttons, ACTIVATE_LABEL, on_activate);
        let deactivate = add_button(page, buttons, DEACTIVATE_LABEL, on_deactivate);

        // Only the managed script offers rules mode; every other script
        // shows this unchecked and insensitive.
        let edit_as_text = gtk_check_button_new_with_mnemonic(translate_static(EDIT_AS_TEXT_LABEL));
        gtk_box_pack_start(content.cast(), edit_as_text, GFALSE, GFALSE, 0);

        // The text-mode editor (item 171a): a GtkSourceView, a GtkTextView
        // subclass, so every call below keeps working against the same
        // pointer. `set_sieve_language` tags its buffer for highlighting.
        let source_view = gtk_source_view_new();
        gtk_text_view_set_monospace(source_view.cast(), GTRUE);
        // Item 180(e): with no completion suggestion showing, Tab moves
        // focus to the next field instead of inserting a tab character;
        // auto-indent takes over the indenting work Tab no longer does.
        gtk_text_view_set_accepts_tab(source_view.cast(), GFALSE);
        gtk_source_view_set_auto_indent(source_view.cast(), GTRUE);
        widget::set_enum_named(source_view.cast(), c"wrap-mode", GTK_WRAP_WORD_CHAR);
        gtk_widget_set_vexpand(source_view, GTRUE);
        let source_label = gtk_label_new_with_mnemonic(translate_static(N_(c"_Script:")));
        gtk_label_set_xalign(source_label.cast(), 0.0);
        gtk_label_set_mnemonic_widget(source_label.cast(), source_view);
        gtk_box_pack_start(content.cast(), source_label, GFALSE, GFALSE, 0);
        gtk_box_pack_start(content.cast(), source_view, GTRUE, GTRUE, 0);
        let buffer = gtk_text_view_get_buffer(source_view.cast());
        set_sieve_language(buffer.cast());

        // Item 171b: a themed error underline for the line a local Sieve
        // parse rejects. One anonymous tag, created once and kept for the
        // page's whole life; `on_script_changed` applies and removes it.
        let error_tag = gtk_text_tag_new(ptr::null());
        widget::set_enum_named(
            error_tag.cast(),
            c"underline",
            widget::PANGO_UNDERLINE_ERROR,
        );
        gtk_text_tag_table_add(gtk_text_buffer_get_tag_table(buffer), error_tag);
        g_signal_connect_data(
            buffer.cast(),
            c"changed".as_ptr(),
            gcallback(on_script_changed as unsafe extern "C" fn(*mut GObject, gpointer)),
            page.cast(),
            None,
            0,
        );

        // Item 171c: the completion popup. A plain `GTK_WINDOW_POPUP`,
        // built once and kept hidden until Ctrl+Space. It never accepts
        // the window manager's own input focus, so every key still
        // reaches `on_source_key_press` below, the one place that reads
        // and writes the popup's own state.
        let completion_window = gtk_window_new(GTK_WINDOW_POPUP);
        gtk_window_set_accept_focus(completion_window.cast(), GFALSE);
        let completion_list = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
        gtk_container_add(completion_window.cast(), completion_list);
        g_signal_connect_data(
            source_view.cast(),
            c"key-press-event".as_ptr(),
            gcallback(
                on_source_key_press
                    as unsafe extern "C" fn(*mut GObject, *mut GdkEventKey, gpointer) -> gboolean,
            ),
            page.cast(),
            None,
            0,
        );

        // The managed script's rules (step B of D3), shown instead of the
        // text view while it parses and the user has not forced text mode.
        let rules_box = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        gtk_box_pack_start(content.cast(), rules_box, GTRUE, GTRUE, 0);

        let mut rule_types = [G_TYPE_STRING, G_TYPE_STRING];
        let rules_store = gtk_list_store_newv(2, rule_types.as_mut_ptr());
        let rules_view = gtk_tree_view_new_with_model(rules_store.cast::<GtkTreeModel>());
        // SAFETY: as for the script list's store above: the view owns the
        // only remaining reference once this one is released.
        g_object_unref(rules_store.cast());
        gtk_tree_view_set_headers_visible(rules_view.cast(), GTRUE);
        widget::set_accessible_name(rules_view.cast(), &translate(RULE_LIST_NAME));
        add_text_column(rules_view, N_(c"Enabled"), RULE_COLUMN_ENABLED, false);
        add_text_column(rules_view, N_(c"Name"), RULE_COLUMN_NAME, true);
        gtk_widget_set_size_request(rules_view, -1, 160);
        gtk_widget_set_hexpand(rules_view, GTRUE);
        gtk_widget_set_vexpand(rules_view, GTRUE);
        gtk_box_pack_start(rules_box.cast(), rules_view, GTRUE, GTRUE, 0);

        let rule_selection = gtk_tree_view_get_selection(rules_view.cast());
        gtk_tree_selection_set_mode(rule_selection, GTK_SELECTION_BROWSE);

        let rule_buttons = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        gtk_box_pack_start(rules_box.cast(), rule_buttons, GFALSE, GFALSE, 0);
        let rule_add = add_button(page, rule_buttons, RULE_ADD_LABEL, on_rule_add);
        let rule_edit = add_button(page, rule_buttons, RULE_EDIT_LABEL, on_rule_edit);
        let rule_remove = add_button(page, rule_buttons, N_(c"Rem_ove"), on_rule_remove);
        let rule_up = add_button(page, rule_buttons, N_(c"Move _Up"), on_rule_up);
        let rule_down = add_button(page, rule_buttons, N_(c"Move Do_wn"), on_rule_down);
        let rule_enable = add_button(page, rule_buttons, N_(c"E_nable"), on_rule_enable);
        let rule_disable = add_button(page, rule_buttons, N_(c"Disa_ble"), on_rule_disable);

        let check = add_button(page, content, N_(c"C_heck"), on_check);

        for control in [
            add,
            rename,
            remove,
            activate,
            deactivate,
            check,
            source_view,
            edit_as_text,
            rule_add,
            rule_edit,
            rule_remove,
            rule_up,
            rule_down,
            rule_enable,
            rule_disable,
        ] {
            gtk_widget_set_sensitive(control, GFALSE);
        }
        gtk_widget_show_all(page.cast());
        gtk_widget_set_visible(rules_box, GFALSE);

        g_signal_connect_data(
            selection.cast(),
            c"changed".as_ptr(),
            gcallback(on_selection_changed as unsafe extern "C" fn(*mut GObject, gpointer)),
            page.cast(),
            None,
            0,
        );
        g_signal_connect_data(
            rule_selection.cast(),
            c"changed".as_ptr(),
            gcallback(on_rule_selection_changed as unsafe extern "C" fn(*mut GObject, gpointer)),
            page.cast(),
            None,
            0,
        );
        g_signal_connect_data(
            edit_as_text.cast(),
            c"toggled".as_ptr(),
            gcallback(on_edit_as_text_toggled as unsafe extern "C" fn(*mut GObject, gpointer)),
            page.cast(),
            None,
            0,
        );

        // Item 180(c): watch the shell's online state, so a page already
        // open and loaded notices the user going offline at once instead
        // of only on the next write. No shell instance (a headless test,
        // or none started yet) leaves both ids at 0: nothing to watch,
        // nothing for `Drop` to disconnect later.
        let shell = e_shell_get_default();
        let (shell_online_handler, shell_network_handler) = if shell.is_null() {
            (0, 0)
        } else {
            (
                g_signal_connect_data(
                    shell.cast(),
                    c"notify::online".as_ptr(),
                    gcallback(
                        on_shell_online_changed
                            as unsafe extern "C" fn(*mut GObject, gpointer, gpointer),
                    ),
                    page.cast(),
                    None,
                    0,
                ),
                g_signal_connect_data(
                    shell.cast(),
                    c"notify::network-available".as_ptr(),
                    gcallback(
                        on_shell_online_changed
                            as unsafe extern "C" fn(*mut GObject, gpointer, gpointer),
                    ),
                    page.cast(),
                    None,
                    0,
                ),
            )
        };

        STATE.set(
            page,
            RefCell::new(UiState {
                link: None,
                form: FiltersForm::default(),
                showing: None,
                rules: None,
                force_text: false,
                syncing: false,
                store,
                view,
                source_label,
                source_view,
                buffer,
                status,
                add,
                rename,
                remove,
                activate,
                deactivate,
                check,
                edit_as_text,
                rules_box,
                rules_store,
                rules_view,
                rule_add,
                rule_edit,
                rule_remove,
                rule_up,
                rule_down,
                rule_enable,
                rule_disable,
                check_generation: 0,
                error_tag,
                validate_timeout: 0,
                saving_text: false,
                completion_window,
                completion_list,
                completion_candidates: Vec::new(),
                completion_selected: 0,
                offline: false,
                shell: shell.cast(),
                shell_online_handler,
                shell_network_handler,
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
        if expand {
            // A long script or rule name must not force the page wider
            // than the window: a fixed sizing mode caps the column's own
            // width request independently of its content, and the
            // ellipsis marks that the name was cut off instead of
            // clipping it with no visual cue.
            widget::set_enum_named(tree_column.cast(), c"sizing", GTK_TREE_VIEW_COLUMN_FIXED);
            widget::set_int_named(tree_column.cast(), c"fixed-width", NAME_COLUMN_WIDTH);
            widget::set_enum_named(renderer.cast(), c"ellipsize", widget::PANGO_ELLIPSIZE_END);
        }
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

    // SAFETY: `create` runs on the main loop, per its own contract (it builds
    // a GTK widget above).
    let offline = unsafe { link::shell_is_offline() };
    // SAFETY: `connect_source` is valid, per the contract of this function.
    // This read happens before any worker thread also touches the source.
    let uid = unsafe { read_string(e_source_get_uid(connect_source)) };
    let cache_dir = cache::cache_dir();
    // SAFETY: the source is valid, per the contract of this function. The
    // worker owns the reference and releases it.
    let source = unsafe { SendSource::adopt(g_object_ref(connect_source.cast()).cast()) };
    let work = move || {
        let outcome = if offline {
            LoadOutcome::Offline
        } else {
            // SAFETY: the reference from `create` keeps the source alive for
            // all of this closure.
            match unsafe { link::connect_account(source.as_ptr()) }.and_then(|link| {
                let loaded = io::load(&link)?;
                Ok((Arc::new(link), loaded))
            }) {
                Ok(loaded) => LoadOutcome::Loaded(loaded),
                Err(message) => LoadOutcome::Failed(message),
            }
        };
        drop(source);
        outcome
    };
    // Item 180(c)'s cold-start cache: `None` when the source had no UID to
    // read, which skips both halves below.
    let cache_slot = uid.map(|uid| (cache_dir, uid));
    let finish = move |page: *mut GObject, outcome| {
        let slot = cache_slot
            .as_ref()
            .map(|(dir, uid)| (dir.as_path(), uid.as_str()));
        // SAFETY: `with_strong` gives the live page.
        unsafe { apply_outcome(page, outcome, slot) };
    };
    // SAFETY: `page` is the live new instance, and this function holds its reference.
    unsafe { dispatch::spawn_for(page, work, finish) };

    page.cast()
}

/// Fills the widgets with the load result, or shows why the load failed.
///
/// `cache`, item 180(c)'s cold-start cache slot (the cache directory and
/// this account's `ESource` UID), is `None` for a page with no UID to key
/// the cache by, or a caller that does not care about it (most tests here).
/// A successful load then saves to it; an offline load with no server
/// connection loads from it and shows what it finds, read-only, instead of
/// just this page's own offline message.
///
/// # Safety
///
/// `page` must be the live page of the load.
unsafe fn apply_outcome(
    page: *mut GObject,
    outcome: LoadOutcome<(Arc<AccountLink>, Loaded)>,
    cache: Option<(&Path, &str)>,
) {
    // SAFETY: the page is live, per the contract.
    let Some(cell) = (unsafe { state(page) }) else {
        return;
    };
    match outcome {
        LoadOutcome::Loaded((link, loaded)) if link.features.sieve => {
            {
                let mut ui = cell.borrow_mut();
                ui.form = FiltersForm::from_loaded(loaded);
                ui.link = Some(link);
            }
            if let Some((dir, uid)) = cache {
                let scripts: Vec<cache::CachedScript> = cell
                    .borrow()
                    .form
                    .entries()
                    .iter()
                    .map(|entry| cache::CachedScript {
                        name: entry.name.clone(),
                        text: entry.text.clone(),
                        active: entry.active,
                    })
                    .collect();
                if let Err(failure) = cache::save(dir, uid, &scripts) {
                    tracing::debug!(%failure, "could not save the Server Filters cold-start cache");
                }
            }
            // SAFETY: the widgets are live children of the page, and no
            // borrow is held across the GTK calls inside.
            unsafe {
                set_status(cell, "");
                refresh(cell);
            }
        }
        LoadOutcome::Loaded(_) => {
            // SAFETY: as above.
            unsafe { set_status(cell, &translate(NOT_OFFERED)) };
        }
        LoadOutcome::Offline => {
            let cached = cache.and_then(|(dir, uid)| cache::load(dir, uid));
            if let Some(scripts) = cached {
                {
                    let mut ui = cell.borrow_mut();
                    ui.form = FiltersForm::from_cached(&scripts);
                    ui.offline = true;
                }
                // SAFETY: as above.
                unsafe {
                    set_status(cell, &translate(OFFLINE));
                    refresh(cell);
                }
            } else {
                // SAFETY: as above.
                unsafe { set_status(cell, &translate(OFFLINE)) };
            }
        }
        LoadOutcome::Failed(message) => {
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
            append_row(store, COLUMN_NAME, name, COLUMN_ACTIVE, &active);
        }
        if let Some(row) = selected {
            set_cursor(view, row);
        }
    }
    cell.borrow_mut().syncing = false;

    // SAFETY: as above.
    unsafe { show_selected(cell) };
}

/// What one call to [`show_selected`] renders: computed once from the form
/// and the staged [`RulesEditor`], then applied to the widgets with no
/// borrow of the state held.
struct ShowPlan {
    script_text: String,
    buttons: Buttons,
    /// True when the managed script parses and the user has not forced text
    /// mode: the rules list replaces the text view.
    want_rules: bool,
    /// The "Edit as text" check button is shown for the managed script only.
    edit_as_text_visible: bool,
    /// Insensitive while the script does not parse: there is no rules view
    /// to switch back to.
    edit_as_text_sensitive: bool,
    edit_as_text_checked: bool,
    /// Set the first time the selection lands on a managed script that does
    /// not parse, so the user learns why rules mode is not available. Later
    /// redraws of the same script leave the status label to whatever the
    /// last action or Check wrote there. Never set once the user has edited
    /// this script's text in this page ([`ScriptEntry::edited_as_text`]): a
    /// text edit made here is not an edit from outside Evolution.
    status_note: Option<&'static CStr>,
    rule_rows: Vec<(String, String)>,
    rule_selected: Option<usize>,
    rule_buttons: rule_form::RuleButtons,
    /// Item 180(c): true while the shell is offline. Every sensitivity flag
    /// below is cleared when this is set, so a selection change made while
    /// offline (which runs this same plan) cannot re-enable a control
    /// [`apply_offline_change`] just disabled.
    offline: bool,
}

/// Item 180(b): true when the external-edit banner ([`EXTERNAL_EDIT`]) should
/// show. Only the first time the selection lands on a managed script that
/// does not parse as rules; a text edit made in this page clears it for
/// good, since that edit is not one from outside Evolution.
fn show_external_edit_note(
    script_changed: bool,
    is_managed: bool,
    can_parse: bool,
    edited_as_text: bool,
) -> bool {
    script_changed && is_managed && !can_parse && !edited_as_text
}

/// Puts the selected script's source in the text view, or the managed
/// script's rules in the rules list, and sets the sensitivity of every
/// control.
///
/// # Safety
///
/// As [`refresh`].
unsafe fn show_selected(cell: &RefCell<UiState>) {
    let plan = {
        let mut ui = cell.borrow_mut();
        let row = ui.form.selected();
        let script_changed = ui.showing != row;
        ui.showing = row;
        if script_changed {
            ui.force_text = false;
            ui.rules = None;
        }

        let entry = row.and_then(|row| ui.form.entries().get(row));
        let script_text = entry.map(|entry| entry.text.clone()).unwrap_or_default();
        let is_managed =
            entry.is_some_and(|entry| entry.name == MANAGED_SCRIPT_NAME && !entry.protected);
        let edited_as_text = entry.is_some_and(|entry| entry.edited_as_text);
        let has_mailboxid = ui.form.has_mailboxid();
        let parsed = is_managed
            .then(|| script::load(&script_text, has_mailboxid))
            .flatten();
        let can_parse = parsed.is_some();
        if can_parse && !ui.force_text {
            if ui.rules.is_none() {
                ui.rules = parsed.map(RulesEditor::from_rules);
            }
        } else {
            ui.rules = None;
        }
        let want_rules = ui.rules.is_some();

        let rule_rows = ui
            .rules
            .as_ref()
            .map(|editor| {
                editor
                    .rules()
                    .iter()
                    .map(|rule| {
                        let enabled = if rule.enabled {
                            translate(ACTIVE_MARK)
                        } else {
                            String::new()
                        };
                        (enabled, rule.name.clone())
                    })
                    .collect()
            })
            .unwrap_or_default();
        let rule_selected = ui.rules.as_ref().and_then(RulesEditor::selected);
        let rule_buttons = ui
            .rules
            .as_ref()
            .map(RulesEditor::buttons)
            .unwrap_or_default();

        ShowPlan {
            script_text,
            buttons: ui.form.buttons(),
            want_rules,
            edit_as_text_visible: is_managed,
            edit_as_text_sensitive: is_managed && can_parse,
            edit_as_text_checked: is_managed && (!can_parse || ui.force_text),
            status_note: show_external_edit_note(
                script_changed,
                is_managed,
                can_parse,
                edited_as_text,
            )
            .then_some(EXTERNAL_EDIT),
            rule_rows,
            rule_selected,
            rule_buttons,
            offline: ui.offline,
        }
    };

    let (
        buffer,
        source_label,
        source_view,
        edit_as_text,
        rules_box,
        rules_store,
        rules_view,
        script_controls,
        rule_controls,
    ) = {
        let ui = cell.borrow();
        (
            ui.buffer,
            ui.source_label,
            ui.source_view,
            ui.edit_as_text,
            ui.rules_box,
            ui.rules_store,
            ui.rules_view,
            [
                ui.add,
                ui.rename,
                ui.remove,
                ui.activate,
                ui.deactivate,
                ui.check,
            ],
            [
                ui.rule_add,
                ui.rule_edit,
                ui.rule_remove,
                ui.rule_up,
                ui.rule_down,
                ui.rule_enable,
                ui.rule_disable,
            ],
        )
    };

    // SAFETY: every widget above is a live child of the page, and no borrow
    // of `cell` is held across these calls.
    unsafe {
        let script_sensitive = [
            plan.buttons.add,
            plan.buttons.rename,
            plan.buttons.remove,
            plan.buttons.activate,
            plan.buttons.deactivate,
            plan.buttons.check,
        ];
        for (control, on) in script_controls.iter().zip(script_sensitive) {
            gtk_widget_set_sensitive(*control, flag(on && !plan.offline));
        }

        gtk_widget_set_visible(source_label, flag(!plan.want_rules));
        gtk_widget_set_visible(source_view, flag(!plan.want_rules));
        gtk_widget_set_sensitive(
            source_view,
            flag(!plan.want_rules && plan.buttons.edit && !plan.offline),
        );
        gtk_text_view_set_editable(source_view.cast(), flag(plan.buttons.edit && !plan.offline));
        set_text(
            buffer.cast(),
            if plan.want_rules {
                ""
            } else {
                &plan.script_text
            },
        );

        gtk_widget_set_visible(rules_box, flag(plan.want_rules));
        gtk_widget_set_sensitive(rule_controls[0], flag(plan.want_rules && !plan.offline));
        let rule_sensitive = [
            plan.want_rules,
            plan.want_rules && plan.rule_buttons.edit,
            plan.want_rules && plan.rule_buttons.remove,
            plan.want_rules && plan.rule_buttons.up,
            plan.want_rules && plan.rule_buttons.down,
            plan.want_rules && plan.rule_buttons.enable,
            plan.want_rules && plan.rule_buttons.disable,
        ];
        for (control, on) in rule_controls.iter().zip(rule_sensitive) {
            gtk_widget_set_sensitive(*control, flag(on && !plan.offline));
        }

        gtk_widget_set_visible(edit_as_text, flag(plan.edit_as_text_visible));
        gtk_widget_set_sensitive(
            edit_as_text,
            flag(plan.edit_as_text_sensitive && !plan.offline),
        );

        // Rewriting the rules store moves its cursor, which emits "changed"
        // on its selection, and `set_bool` below emits "toggled" when the
        // value changes. Both handlers stand down while this flag is set, as
        // the script list's own rewrite in `refresh` already relies on.
        cell.borrow_mut().syncing = true;
        set_bool(edit_as_text.cast(), plan.edit_as_text_checked);
        gtk_list_store_clear(rules_store);
        for (enabled, name) in &plan.rule_rows {
            append_row(
                rules_store,
                RULE_COLUMN_ENABLED,
                enabled,
                RULE_COLUMN_NAME,
                name,
            );
        }
        if let Some(row) = plan.rule_selected {
            set_cursor(rules_view, row);
        }
        cell.borrow_mut().syncing = false;

        if let Some(note) = plan.status_note {
            set_status(cell, &translate(note));
        }
    }
}

/// Item 180(c)'s "went offline while the page was open" half. Flushes the
/// buffer into the form first, so going offline never loses a keystroke
/// made just before it (the buffer and the form stay in step either way,
/// since neither this function nor the later [`show_selected`] ever writes
/// one from a stale copy of the other). Then redraws through
/// [`show_selected`], whose own sensitivity computation now clears every
/// control while [`UiState::offline`] is set, and sets or clears the
/// offline note. A page whose initial load never finished (`link` is
/// `None`) ignores this: it already shows its own offline message, and a
/// cold start showing the last loaded scripts read-only is a separate,
/// still open, slice.
///
/// # Safety
///
/// As [`show_selected`].
unsafe fn apply_offline_change(cell: &RefCell<UiState>, offline: bool) {
    {
        let mut ui = cell.borrow_mut();
        if ui.link.is_none() || ui.offline == offline {
            return;
        }
        ui.offline = offline;
    }
    let note = if offline {
        translate(OFFLINE)
    } else {
        String::new()
    };
    // SAFETY: as `show_selected`.
    unsafe {
        flush(cell);
        show_selected(cell);
        set_status(cell, &note);
    }
}

/// The shell's `notify::online` and `notify::network-available` handler
/// (item 180(c)). Both signals share it: either firing is reason enough to
/// recheck [`link::shell_is_offline`] and apply the change, if any.
///
/// # Safety
///
/// The GLib signal system calls this with the page as the data pointer.
unsafe extern "C" fn on_shell_online_changed(
    _shell: *mut GObject,
    _pspec: gpointer,
    page: gpointer,
) {
    guard("JmapFiltersPage::shell-notify", (), || {
        // SAFETY: the page is live: `Drop` disconnects this handler before
        // the page is freed.
        let Some(cell) = (unsafe { state(page.cast()) }) else {
            return;
        };
        // SAFETY: this handler runs on the main loop, the same thread that
        // writes the shell's `online` and `network-available` properties.
        let offline = unsafe { link::shell_is_offline() };
        // SAFETY: as `show_selected`.
        unsafe { apply_offline_change(cell, offline) };
    });
}

/// Copies the text view back into the script it belongs to. Every action
/// calls this first, because the page's own buffer signal (item 171b) only
/// debounces a local syntax check, not staging into the form. A no-op while
/// rules mode shows the managed script: its text is not the buffer's, and
/// the buffer holds nothing to lose.
///
/// # Safety
///
/// As [`refresh`].
unsafe fn flush(cell: &RefCell<UiState>) {
    let (buffer, showing) = {
        let ui = cell.borrow();
        if ui.rules.is_some() {
            return;
        }
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

/// The `changed` handler of the text-mode editor's buffer (item 171b). Every
/// edit restarts a debounce timeout; only the last edit in a burst runs the
/// local Sieve parse.
///
/// # Safety
///
/// The GLib signal system calls this with the page as the data pointer.
unsafe extern "C" fn on_script_changed(_buffer: *mut GObject, page: gpointer) {
    guard("JmapFiltersPage::script-changed", (), || {
        // SAFETY: the page is live, because its buffer emitted.
        let Some(cell) = (unsafe { state(page.cast()) }) else {
            return;
        };
        let pending = std::mem::replace(&mut cell.borrow_mut().validate_timeout, 0);
        if pending != 0 {
            // SAFETY: a source id this page added, not yet fired (it would
            // have cleared the field itself).
            unsafe { g_source_remove(pending) };
        }
        // SAFETY: `page` is live for the duration of this call; the timeout
        // either fires on this same main loop while it still is, or this
        // page's `Drop` cancels it first.
        let id = unsafe {
            g_timeout_add(
                VALIDATE_DEBOUNCE_MS,
                Some(on_validate_timeout as unsafe extern "C" fn(gpointer) -> gboolean),
                page,
            )
        };
        cell.borrow_mut().validate_timeout = id;
    });
}

/// The debounce timeout `on_script_changed` schedules. Runs the local Sieve
/// parse over the whole script and marks the line of its first problem, if
/// any, with a themed error underline and a tooltip of the message, drives
/// completion while typing (item 180(e)), and, on a script that parses,
/// saves its text (item 180(a), [`maybe_save_text`]). Fires at most once per
/// burst of edits.
///
/// # Safety
///
/// `page` is the data pointer `on_script_changed` gave `g_timeout_add`.
unsafe extern "C" fn on_validate_timeout(page: gpointer) -> gboolean {
    guard("JmapFiltersPage::validate-timeout", G_SOURCE_REMOVE, || {
        let page = page.cast::<GObject>();
        // SAFETY: the page is live: this callback runs on the main loop this
        // page's own `Drop` cancels it from before it is freed.
        let Some(cell) = (unsafe { state(page) }) else {
            return G_SOURCE_REMOVE;
        };
        // The timeout fired: it is no longer pending.
        cell.borrow_mut().validate_timeout = 0;
        let (buffer, source_view, error_tag) = {
            let ui = cell.borrow();
            (ui.buffer, ui.source_view, ui.error_tag)
        };
        // SAFETY: every widget is a live child of the page, and no borrow of
        // `cell` is held across these calls.
        let problem = unsafe {
            let text = get_text(buffer.cast());
            let mut start = GtkTextIter::default();
            let mut end = GtkTextIter::default();
            gtk_text_buffer_get_start_iter(buffer, &mut start);
            gtk_text_buffer_get_end_iter(buffer, &mut end);
            gtk_text_buffer_remove_tag(buffer, error_tag, &start, &end);
            let problem = validate::find_problem(&text);
            match &problem {
                Some(problem) => {
                    let line = problem.line.saturating_sub(1) as c_int;
                    let mut line_start = GtkTextIter::default();
                    gtk_text_buffer_get_iter_at_line(buffer, &mut line_start, line);
                    let mut line_end = line_start;
                    gtk_text_iter_forward_to_line_end(&mut line_end);
                    gtk_text_buffer_apply_tag(buffer, error_tag, &line_start, &line_end);
                    let message = cstring_lossy(&problem.message);
                    gtk_widget_set_tooltip_text(source_view.cast(), message.as_ptr());
                }
                None => gtk_widget_set_tooltip_text(source_view.cast(), ptr::null()),
            }
            problem
        };
        // Item 180(e): the same debounce also drives completion while
        // typing, with no Ctrl+Space needed. `show_completion_for_word`
        // refreshes the popup for the word now under the cursor; if that
        // word is too short or matches nothing, close whatever the last
        // edit left open rather than show a stale list.
        // SAFETY: as `show_completion_for_word`'s and `close_completion`'s
        // own contracts.
        unsafe {
            if !show_completion_for_word(cell, AUTO_COMPLETE_MIN_CHARS) {
                close_completion(cell);
            }
        }
        // Item 180(a)'s last open row: a script that parses locally also
        // gets its text saved now, instead of only on the account editor's
        // OK. The debounce is the chosen trigger over "leaving the script":
        // it needs no new signal connection and already fires on every
        // pause in typing, so a script left open and untouched still saves.
        // SAFETY: as `maybe_save_text`.
        if problem.is_none() {
            unsafe { maybe_save_text(cell, page) };
        }
        G_SOURCE_REMOVE
    })
}

/// Saves the script text at the earliest safe point (item 180(a)) instead of
/// waiting for the account editor's OK: [`on_validate_timeout`] calls this
/// once the local Sieve parse finds no problem. The server's own validation
/// inside [`io::save`] (through [`io::activate_now`]) still decides whether
/// anything is written, so a script invalid only server-side (RFC 9661
/// Section 2.5) is refused the same way the Check button refuses it, and
/// the buffer keeps the user's text either way: nothing here reverts it.
///
/// Reuses [`io::activate_now`]'s write-and-reload round trip, the same one
/// the Activate, Deactivate, Rename, Add and Remove buttons use, so any
/// other change already staged on another row saves at the same time, same
/// as those buttons do today.
///
/// # Safety
///
/// `cell` is the state of the live page `page` points to.
unsafe fn maybe_save_text(cell: &RefCell<UiState>, page: *mut GObject) {
    if cell.borrow().saving_text {
        // A save from an earlier debounce is still in flight: reschedule
        // instead of dropping this edit, so it is retried once that one
        // answers.
        // SAFETY: `page` is live for the duration of this call; the timeout
        // either fires on this same main loop while it still is, or this
        // page's `Drop` cancels it first.
        let id = unsafe {
            g_timeout_add(
                VALIDATE_DEBOUNCE_MS,
                Some(on_validate_timeout as unsafe extern "C" fn(gpointer) -> gboolean),
                page.cast(),
            )
        };
        cell.borrow_mut().validate_timeout = id;
        return;
    }
    // SAFETY: the widgets are live, and no borrow is held.
    unsafe { flush(cell) };
    let Some(row) = cell.borrow().form.selected() else {
        return;
    };
    let planned = {
        let ui = cell.borrow();
        (ui.link.clone(), ui.form.plan())
    };
    let (link, plan) = match planned {
        (_, Ok(None)) | (None, Ok(Some(_))) => return,
        (_, Err(reason)) => {
            // SAFETY: as above.
            unsafe { set_status(cell, &reason) };
            return;
        }
        (Some(link), Ok(Some(plan))) => (link, plan),
    };

    cell.borrow_mut().saving_text = true;
    // SAFETY: the widgets are live, and no borrow is held.
    unsafe {
        set_script_controls_sensitive(cell, false);
        set_status(cell, &translate(SAVING_TEXT));
    }

    let work = move || io::activate_now(&link, &plan);
    let finish = move |page: *mut GObject, outcome: Result<ActivateOutcome, String>| {
        // SAFETY: `with_strong` gives the live page.
        let Some(cell) = (unsafe { state(page) }) else {
            return;
        };
        cell.borrow_mut().saving_text = false;
        match outcome {
            Ok(ActivateOutcome::Reloaded(loaded)) => {
                let name = cell
                    .borrow()
                    .form
                    .entries()
                    .get(row)
                    .map(|entry| entry.name.clone());
                {
                    let mut ui = cell.borrow_mut();
                    ui.form = FiltersForm::from_loaded(loaded);
                    if let Some(name) = name {
                        let found = ui.form.entries().iter().position(|e| e.name == name);
                        ui.form.select(found);
                    }
                }
                // SAFETY: as above.
                unsafe {
                    set_status(cell, "");
                    refresh(cell);
                }
            }
            Ok(ActivateOutcome::WrittenReloadFailed(reason)) => {
                let text = translate_with(SAVE_TEXT_RELOAD_FAILED, &[&reason]);
                // SAFETY: as above.
                unsafe {
                    set_status(cell, &text);
                    refresh(cell);
                }
            }
            Err(reason) => {
                let text = translate_with(SAVE_TEXT_FAILED, &[&reason]);
                // SAFETY: as above.
                unsafe {
                    set_status(cell, &text);
                    refresh(cell);
                }
            }
        }
    };
    // SAFETY: `page` is live, the same invariant `on_validate_timeout` relies
    // on: this page's own `Drop` cancels the debounce before freeing it, and
    // this call runs from that debounce.
    unsafe { dispatch::spawn_for(page, work, finish) };
}

/// Item 171c's completion popup. The text before `iter` on `iter`'s own
/// line, the slice [`completion::word_under_cursor`] wants.
///
/// # Safety
///
/// `buffer` is a live buffer; `iter` is one of its own, valid iters.
unsafe fn line_prefix(buffer: *mut GtkTextBuffer, iter: &GtkTextIter) -> String {
    // SAFETY: per the contract. The returned string is a fresh GLib
    // allocation this call frees through `take_string`.
    unsafe {
        let mut line_start = GtkTextIter::default();
        gtk_text_buffer_get_iter_at_line(buffer, &mut line_start, gtk_text_iter_get_line(iter));
        let raw = gtk_text_buffer_get_text(buffer, &line_start, iter, GFALSE);
        take_string(raw).unwrap_or_default()
    }
}

/// Replaces the identifier word ending at `cursor` with `replacement`,
/// placing the cursor mark right after it (item 171c). The core of
/// [`accept_completion`], factored out so a test can drive it against a
/// bare `GtkTextBuffer` with no display.
///
/// # Safety
///
/// `buffer` is a live buffer; `cursor` is one of its own, valid iters, and
/// nothing else holds a borrow of it.
unsafe fn replace_word_at(buffer: *mut GtkTextBuffer, cursor: &GtkTextIter, replacement: &str) {
    // SAFETY: per the contract. `word`'s characters are all ASCII (see
    // `completion::word_under_cursor`), so its `char` count is exactly how
    // far back to move to reach its first character, with no byte/char
    // distinction to get wrong even though the rest of the line may hold
    // multi-byte UTF-8 this never has to measure.
    unsafe {
        let prefix = line_prefix(buffer, cursor);
        let word = completion::word_under_cursor(&prefix);
        let mut word_start = *cursor;
        gtk_text_iter_backward_chars(&mut word_start, word.chars().count() as c_int);
        let mut cursor_copy = *cursor;
        let text = cstring_lossy(replacement);
        gtk_text_buffer_delete(buffer, &mut word_start, &mut cursor_copy);
        gtk_text_buffer_insert(buffer, &mut word_start, text.as_ptr(), -1);
        gtk_text_buffer_place_cursor(buffer, &word_start);
    }
}

/// Rebuilds the popup's label list from `UiState`'s own candidate list and
/// selection (item 171c). Destroys the previous labels first: GTK has no
/// "replace all children" call, and the list is short enough (at most
/// [`MAX_COMPLETION_ROWS`]) that a full rebuild on every Up/Down keeps this
/// simple. The selected row is marked with a leading arrow, not a colour,
/// so the dark and High Contrast themes keep working (item 174c).
///
/// # Safety
///
/// As [`refresh`].
unsafe fn fill_completion_list(cell: &RefCell<UiState>) {
    let (list, candidates, selected) = {
        let ui = cell.borrow();
        (
            ui.completion_list,
            ui.completion_candidates.clone(),
            ui.completion_selected,
        )
    };
    // SAFETY: `list` is a live child of the page's own completion window;
    // no borrow of `cell` is held across these calls.
    unsafe {
        let children = gtk_container_get_children(list.cast());
        let mut node = children;
        while !node.is_null() {
            gtk_widget_destroy((*node).data.cast());
            node = (*node).next;
        }
        g_list_free(children);
        for (index, candidate) in candidates.iter().enumerate() {
            let marker = if index == selected { "▸ " } else { "  " };
            let label = gtk_label_new(ptr::null());
            gtk_label_set_xalign(label.cast(), 0.0);
            let text = cstring_lossy(&format!("{marker}{}", candidate.text));
            gtk_label_set_text(label.cast(), text.as_ptr());
            gtk_box_pack_start(list.cast(), label, GFALSE, GFALSE, 0);
        }
        gtk_widget_show_all(list);
    }
}

/// Moves the popup to just below the line `cursor` is on, in screen
/// coordinates (item 171c).
///
/// # Safety
///
/// `source_view` must be realised (a live child of a mapped page): a key
/// press cannot otherwise reach `on_source_key_press` to call this.
unsafe fn position_completion_window(
    window: *mut GtkWidget,
    source_view: *mut GtkWidget,
    cursor: &GtkTextIter,
) {
    // SAFETY: per the contract.
    unsafe {
        let mut location: GdkRectangle = std::mem::zeroed();
        gtk_text_view_get_iter_location(source_view.cast(), cursor, &mut location);
        let mut window_x: c_int = 0;
        let mut window_y: c_int = 0;
        gtk_text_view_buffer_to_window_coords(
            source_view.cast(),
            GTK_TEXT_WINDOW_WIDGET,
            location.x,
            location.y,
            &mut window_x,
            &mut window_y,
        );
        let mut origin_x: c_int = 0;
        let mut origin_y: c_int = 0;
        gdk_window_get_origin(
            gtk_widget_get_window(source_view),
            &mut origin_x,
            &mut origin_y,
        );
        gtk_window_move(
            window.cast(),
            origin_x + window_x,
            origin_y + window_y + location.height,
        );
    }
}

/// The word under the cursor must be at least this many characters before
/// typing on its own opens the completion popup (item 180(e)). Below this,
/// the popup only ever opens from the explicit Ctrl+Space trigger
/// ([`open_completion`], which passes 0 here instead).
const AUTO_COMPLETE_MIN_CHARS: usize = 2;

/// Shows the completion popup for the word under the cursor, if it has at
/// least `min_chars` characters and at least one candidate matches it.
/// Returns whether the popup now shows. Shared by [`open_completion`]
/// (item 171c, Ctrl+Space, `min_chars` 0: any word, even empty, that
/// matches something) and [`on_validate_timeout`] (item 180(e), typing,
/// [`AUTO_COMPLETE_MIN_CHARS`]: never steal keys while the word being typed
/// is still too short to narrow the list usefully).
///
/// # Safety
///
/// As [`refresh`].
unsafe fn show_completion_for_word(cell: &RefCell<UiState>, min_chars: usize) -> bool {
    let (buffer, source_view, extensions) = {
        let ui = cell.borrow();
        (ui.buffer, ui.source_view, ui.form.extensions().to_vec())
    };
    // SAFETY: both widgets are live children of the page; no borrow of
    // `cell` is held across these calls.
    unsafe {
        let mark = gtk_text_buffer_get_insert(buffer);
        let mut cursor = GtkTextIter::default();
        gtk_text_buffer_get_iter_at_mark(buffer, &mut cursor, mark);
        let prefix = line_prefix(buffer, &cursor);
        let word = completion::word_under_cursor(&prefix);
        if word.chars().count() < min_chars {
            return false;
        }
        let matches: Vec<&'static completion::Candidate> =
            completion::candidates(word, &extensions)
                .take(MAX_COMPLETION_ROWS)
                .collect();
        if matches.is_empty() {
            return false;
        }
        tracing::trace!(
            word,
            candidate_count = matches.len(),
            "sieve completion: popup opened"
        );
        {
            let mut ui = cell.borrow_mut();
            ui.completion_candidates = matches;
            ui.completion_selected = 0;
        }
        fill_completion_list(cell);
        let window = cell.borrow().completion_window;
        position_completion_window(window, source_view, &cursor);
        gtk_widget_show_all(window);
        true
    }
}

/// Opens the completion popup near the cursor (item 171c, Ctrl+Space).
/// Does nothing if no candidate matches the word being typed: an empty
/// popup would only be a key away from closing again anyway.
///
/// # Safety
///
/// As [`refresh`].
unsafe fn open_completion(cell: &RefCell<UiState>) {
    // SAFETY: as `show_completion_for_word`'s own contract.
    unsafe { show_completion_for_word(cell, 0) };
}

/// Hides the popup and clears its candidate list (item 171c). Safe to call
/// when it is already hidden.
///
/// # Safety
///
/// As [`refresh`].
unsafe fn close_completion(cell: &RefCell<UiState>) {
    let (window, was_open) = {
        let mut ui = cell.borrow_mut();
        let was_open = !ui.completion_candidates.is_empty();
        ui.completion_candidates.clear();
        ui.completion_selected = 0;
        (ui.completion_window, was_open)
    };
    if was_open {
        tracing::trace!("sieve completion: popup closed");
    }
    // SAFETY: a live child of the page.
    unsafe { gtk_widget_hide(window) };
}

/// Moves the popup's own selection by `delta` rows, wrapping at either end
/// (item 171c, Up/Down). Does nothing if the popup is closed.
///
/// # Safety
///
/// As [`refresh`].
unsafe fn move_completion_selection(cell: &RefCell<UiState>, delta: i32) {
    {
        let mut ui = cell.borrow_mut();
        let len = ui.completion_candidates.len();
        if len == 0 {
            return;
        }
        let selected = ui.completion_selected as i32 + delta;
        ui.completion_selected = selected.rem_euclid(len as i32) as usize;
    }
    // SAFETY: as `fill_completion_list`'s own contract.
    unsafe { fill_completion_list(cell) };
}

/// Replaces the word under the cursor with the selected candidate and
/// closes the popup (item 171c, Tab/Enter).
///
/// # Safety
///
/// As [`refresh`].
unsafe fn accept_completion(cell: &RefCell<UiState>) {
    let accepted = {
        let ui = cell.borrow();
        let buffer = ui.buffer;
        ui.completion_candidates
            .get(ui.completion_selected)
            .map(|candidate| (buffer, candidate.text))
    };
    if let Some((buffer, text)) = accepted {
        tracing::trace!(accepted = text, "sieve completion: candidate accepted");
        // SAFETY: `buffer` is a live child of the page.
        unsafe {
            let mark = gtk_text_buffer_get_insert(buffer);
            let mut cursor = GtkTextIter::default();
            gtk_text_buffer_get_iter_at_mark(buffer, &mut cursor, mark);
            replace_word_at(buffer, &cursor, text);
        }
    }
    // SAFETY: as `close_completion`'s own contract.
    unsafe { close_completion(cell) };
}

/// What one key press asks the completion popup to do (item 171c, the
/// Right arrow row added by item 180(e)). Pure dispatch on the keyval and
/// modifiers alone, factored out of [`on_source_key_press`] so a test can
/// drive it with no live `GdkEventKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionKey {
    MoveSelection(i32),
    Accept,
    Close,
    /// Closes the popup and lets the keystroke still reach the buffer.
    CloseAndPropagate,
    Open,
    /// Not a completion key: let the buffer handle it as usual.
    Ignore,
}

// `GDK_KEY_*` are bindgen's own names for `gdk/gdkkeysyms.h`'s macros,
// matched as patterns below: GDK's own mixed-case spelling, not a local
// naming choice to fix.
#[allow(non_upper_case_globals)]
fn completion_key_action(keyval: c_uint, mod_state: c_uint, showing: bool) -> CompletionKey {
    if showing {
        match keyval {
            GDK_KEY_Up => CompletionKey::MoveSelection(-1),
            GDK_KEY_Down => CompletionKey::MoveSelection(1),
            // Right arrow accepts the same way Tab and Enter do (item
            // 180(e)): the popup only ever shows with the cursor right
            // after the word it matched, so there is no "elsewhere" case
            // to tell apart from this one while it is open.
            GDK_KEY_Tab | GDK_KEY_Return | GDK_KEY_Right => CompletionKey::Accept,
            GDK_KEY_Escape => CompletionKey::Close,
            _ => CompletionKey::CloseAndPropagate,
        }
    } else if keyval == GDK_KEY_space && (mod_state & GDK_CONTROL_MASK) != 0 {
        CompletionKey::Open
    } else {
        CompletionKey::Ignore
    }
}

/// The text-mode editor's own `key-press-event` handler (item 171c). While
/// the completion popup is open, Up/Down move its selection, Tab/Enter/
/// Right arrow accepts the selected candidate (item 180(e)), Escape closes
/// it, and any other key closes it without consuming the event, so the
/// keystroke still reaches the buffer normally. While it is closed,
/// Ctrl+Space opens it, and every other key reaches the buffer as usual,
/// Right arrow included.
///
/// # Safety
///
/// The GLib signal system calls this with the page as the data pointer and
/// a live `GdkEventKey` as the event.
unsafe extern "C" fn on_source_key_press(
    _widget: *mut GObject,
    event: *mut GdkEventKey,
    page: gpointer,
) -> gboolean {
    guard("JmapFiltersPage::key-press", GFALSE, || {
        // SAFETY: the view is live, because it emitted the signal.
        let Some(cell) = (unsafe { state(page.cast()) }) else {
            return GFALSE;
        };
        let mut keyval: c_uint = 0;
        let mut mod_state: c_uint = 0;
        // SAFETY: `event` is the live `GdkEventKey` this signal emission
        // passed. `GdkEventKey` is one member of `GdkEvent`'s own C union,
        // sharing its common header fields, so this cast is the same one
        // every GTK application reading a key event this way makes.
        unsafe {
            gdk_event_get_keyval(event.cast(), &mut keyval);
            gdk_event_get_state(event.cast(), &mut mod_state);
        }
        let showing = !cell.borrow().completion_candidates.is_empty();
        match completion_key_action(keyval, mod_state, showing) {
            CompletionKey::MoveSelection(delta) => {
                unsafe { move_completion_selection(cell, delta) };
                GTRUE
            }
            CompletionKey::Accept => {
                unsafe { accept_completion(cell) };
                GTRUE
            }
            CompletionKey::Close => {
                unsafe { close_completion(cell) };
                GTRUE
            }
            CompletionKey::CloseAndPropagate => {
                unsafe { close_completion(cell) };
                GFALSE
            }
            CompletionKey::Open => {
                unsafe { open_completion(cell) };
                GTRUE
            }
            CompletionKey::Ignore => GFALSE,
        }
    })
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

/// Appends one row of two string columns to a list store: `first` in
/// `first_column`, `second` in `second_column`.
///
/// # Safety
///
/// `store` must be the live two-string-column store of a live page.
unsafe fn append_row(
    store: *mut GtkListStore,
    first_column: c_int,
    first: &str,
    second_column: c_int,
    second: &str,
) {
    let first = cstring_lossy(first);
    let second = cstring_lossy(second);
    let mut columns = [first_column, second_column];
    // SAFETY: a zeroed GValue is `G_VALUE_INIT`, the state `g_value_init`
    // requires. Both values are strings, which the store's two columns hold,
    // and both are released below. NULL as the iterator means "do not report
    // the new row back", which this caller does not need.
    unsafe {
        let mut values: [GValue; 2] = [std::mem::zeroed(), std::mem::zeroed()];
        g_value_init(&mut values[0], G_TYPE_STRING);
        g_value_set_string(&mut values[0], first.as_ptr());
        g_value_init(&mut values[1], G_TYPE_STRING);
        g_value_set_string(&mut values[1], second.as_ptr());
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

/// The `changed` handler of the rules list selection.
///
/// # Safety
///
/// As [`on_selection_changed`], for the rules list.
unsafe extern "C" fn on_rule_selection_changed(selection: *mut GObject, page: gpointer) {
    guard("JmapFiltersPage::rule-selection-changed", (), || {
        // SAFETY: the page is live, because its descendant emitted.
        let Some(cell) = (unsafe { state(page.cast()) }) else {
            return;
        };
        if cell.borrow().syncing {
            return;
        }
        // SAFETY: the selection is live, and no borrow of `cell` is held
        // across this call.
        let row = unsafe { selected_row(selection.cast()) };
        {
            let mut ui = cell.borrow_mut();
            if let Some(rules) = ui.rules.as_mut() {
                rules.select(row);
            }
        }
        // SAFETY: as above.
        unsafe { show_selected(cell) };
    });
}

/// The `toggled` handler of the "Edit as text" check button.
///
/// # Safety
///
/// The GLib signal system calls this. `page` is the data pointer of the
/// connection: the page of the button.
unsafe extern "C" fn on_edit_as_text_toggled(_button: *mut GObject, page: gpointer) {
    guard("JmapFiltersPage::edit-as-text-toggled", (), || {
        // SAFETY: the page is live, because its child emitted.
        let Some(cell) = (unsafe { state(page.cast()) }) else {
            return;
        };
        if cell.borrow().syncing {
            return;
        }
        // SAFETY: the widgets are live, and no borrow is held. `flush`
        // writes back a hand edit the user made before the click; it is a
        // no-op while rules mode was showing, so this never overwrites the
        // managed script's text with the hidden, stale buffer.
        unsafe { flush(cell) };
        let checked = {
            let ui = cell.borrow();
            // SAFETY: the check button is a live child of the page.
            unsafe { get_bool(ui.edit_as_text.cast()) }
        };
        {
            let mut ui = cell.borrow_mut();
            ui.force_text = checked;
            // Forces a fresh parse of the current text: turning rules mode
            // back on must not keep an editor built before this toggle.
            ui.rules = None;
        }
        // SAFETY: as above.
        unsafe { show_selected(cell) };
    });
}

unsafe extern "C" fn on_add(_button: *mut GObject, page: gpointer) {
    // SAFETY: the page is live, because its button emitted.
    unsafe { add_now(page) };
}

unsafe extern "C" fn on_rename(_button: *mut GObject, page: gpointer) {
    // SAFETY: as above.
    unsafe { rename_now(page) };
}

unsafe extern "C" fn on_remove(_button: *mut GObject, page: gpointer) {
    // SAFETY: as above.
    unsafe { remove_now(page) };
}

unsafe extern "C" fn on_activate(_button: *mut GObject, page: gpointer) {
    // SAFETY: as above.
    unsafe { activate_now(page) };
}

/// Activates the selected script at once instead of staging it for the
/// account editor's OK (item 180(a)): stages the activation, writes the
/// account's current plan on a worker thread, and reloads the list from the
/// server, the same worker round trip the Delegates page's Add and Revoke
/// use. The script list and buttons stay insensitive meanwhile. A write the
/// server refuses restores the previous activation; nothing else staged is
/// touched either way.
///
/// # Safety
///
/// `page` must be the live page the Activate button belongs to.
unsafe fn activate_now(page: gpointer) {
    guard("JmapFiltersPage::activate", (), || {
        // SAFETY: the page is live, because its button emitted.
        let Some(cell) = (unsafe { state(page.cast()) }) else {
            return;
        };
        // SAFETY: the widgets are live, and no borrow is held.
        unsafe { flush(cell) };
        let Some(row) = cell.borrow().form.selected() else {
            return refuse(cell, NO_SCRIPT_SELECTED);
        };
        let previous_active = cell.borrow().form.active_row();
        if let Err(reason) = cell.borrow_mut().form.activate(row) {
            return refuse(cell, reason);
        }

        let planned = {
            let ui = cell.borrow();
            (ui.link.clone(), ui.form.plan())
        };
        let (link, plan) = match planned {
            // The activation above always makes the form dirty, so a `None`
            // plan or link is not expected; leave nothing written either way.
            (_, Ok(None)) | (None, Ok(Some(_))) => {
                cell.borrow_mut().form.restore_active(previous_active);
                // SAFETY: as above.
                unsafe { refresh(cell) };
                return;
            }
            (_, Err(reason)) => {
                cell.borrow_mut().form.restore_active(previous_active);
                let text = translate_with(ACTIVATE_FAILED, &[&reason]);
                // SAFETY: as above.
                unsafe {
                    set_status(cell, &text);
                    refresh(cell);
                }
                return;
            }
            (Some(link), Ok(Some(plan))) => (link, plan),
        };

        // SAFETY: the widgets are live, and no borrow is held.
        unsafe {
            set_script_controls_sensitive(cell, false);
            set_status(cell, &translate(ACTIVATING));
        }

        let work = move || io::activate_now(&link, &plan);
        let finish = move |page: *mut GObject, outcome: Result<ActivateOutcome, String>| {
            // SAFETY: `with_strong` gives the live page.
            let Some(cell) = (unsafe { state(page) }) else {
                return;
            };
            match outcome {
                Ok(ActivateOutcome::Reloaded(loaded)) => {
                    let name = cell
                        .borrow()
                        .form
                        .entries()
                        .get(row)
                        .map(|entry| entry.name.clone());
                    {
                        let mut ui = cell.borrow_mut();
                        ui.form = FiltersForm::from_loaded(loaded);
                        if let Some(name) = name {
                            let found = ui.form.entries().iter().position(|e| e.name == name);
                            ui.form.select(found);
                        }
                    }
                    // SAFETY: as above.
                    unsafe {
                        set_status(cell, "");
                        refresh(cell);
                    }
                }
                Ok(ActivateOutcome::WrittenReloadFailed(reason)) => {
                    let text = translate_with(ACTIVATE_RELOAD_FAILED, &[&reason]);
                    // SAFETY: as above.
                    unsafe {
                        set_status(cell, &text);
                        refresh(cell);
                    }
                }
                Err(reason) => {
                    cell.borrow_mut().form.restore_active(previous_active);
                    let text = translate_with(ACTIVATE_FAILED, &[&reason]);
                    // SAFETY: as above.
                    unsafe {
                        set_status(cell, &text);
                        refresh(cell);
                    }
                }
            }
        };
        // SAFETY: the page is live and the signal emission holds a reference
        // to it for all of this call.
        unsafe { dispatch::spawn_for(page.cast(), work, finish) };
    });
}

/// Sets every script-list control's sensitivity, overriding whatever the
/// form's own [`crate::filters::form::Buttons`] last computed. Used while an
/// instant-apply write (item 180(a)) is in flight; the next [`refresh`]
/// restores the real sensitivity once it answers.
///
/// # Safety
///
/// As [`refresh`].
unsafe fn set_script_controls_sensitive(cell: &RefCell<UiState>, sensitive: bool) {
    let controls = {
        let ui = cell.borrow();
        [
            ui.add,
            ui.rename,
            ui.remove,
            ui.activate,
            ui.deactivate,
            ui.check,
        ]
    };
    for control in controls {
        // SAFETY: every widget is a live child of the page.
        unsafe { gtk_widget_set_sensitive(control, flag(sensitive)) };
    }
}

unsafe extern "C" fn on_deactivate(_button: *mut GObject, page: gpointer) {
    // SAFETY: as above.
    unsafe { deactivate_now(page) };
}

/// Deactivates the selected script at once instead of staging it for the
/// account editor's OK (item 180(a)): mirrors [`activate_now`] exactly,
/// down to the worker round trip and the control-disable while it runs. A
/// write the server refuses restores the previous activation; nothing else
/// staged is touched either way.
///
/// # Safety
///
/// `page` must be the live page the Deactivate button belongs to.
unsafe fn deactivate_now(page: gpointer) {
    guard("JmapFiltersPage::deactivate", (), || {
        // SAFETY: the page is live, because its button emitted.
        let Some(cell) = (unsafe { state(page.cast()) }) else {
            return;
        };
        // SAFETY: the widgets are live, and no borrow is held.
        unsafe { flush(cell) };
        let Some(row) = cell.borrow().form.selected() else {
            return refuse(cell, NO_SCRIPT_SELECTED);
        };
        let previous_active = cell.borrow().form.active_row();
        if let Err(reason) = cell.borrow_mut().form.deactivate(row) {
            return refuse(cell, reason);
        }

        let planned = {
            let ui = cell.borrow();
            (ui.link.clone(), ui.form.plan())
        };
        let (link, plan) = match planned {
            // The deactivation above always makes the form dirty, so a
            // `None` plan or link is not expected; leave nothing written
            // either way.
            (_, Ok(None)) | (None, Ok(Some(_))) => {
                cell.borrow_mut().form.restore_active(previous_active);
                // SAFETY: as above.
                unsafe { refresh(cell) };
                return;
            }
            (_, Err(reason)) => {
                cell.borrow_mut().form.restore_active(previous_active);
                let text = translate_with(DEACTIVATE_FAILED, &[&reason]);
                // SAFETY: as above.
                unsafe {
                    set_status(cell, &text);
                    refresh(cell);
                }
                return;
            }
            (Some(link), Ok(Some(plan))) => (link, plan),
        };

        // SAFETY: the widgets are live, and no borrow is held.
        unsafe {
            set_script_controls_sensitive(cell, false);
            set_status(cell, &translate(DEACTIVATING));
        }

        let work = move || io::activate_now(&link, &plan);
        let finish = move |page: *mut GObject, outcome: Result<ActivateOutcome, String>| {
            // SAFETY: `with_strong` gives the live page.
            let Some(cell) = (unsafe { state(page) }) else {
                return;
            };
            match outcome {
                Ok(ActivateOutcome::Reloaded(loaded)) => {
                    let name = cell
                        .borrow()
                        .form
                        .entries()
                        .get(row)
                        .map(|entry| entry.name.clone());
                    {
                        let mut ui = cell.borrow_mut();
                        ui.form = FiltersForm::from_loaded(loaded);
                        if let Some(name) = name {
                            let found = ui.form.entries().iter().position(|e| e.name == name);
                            ui.form.select(found);
                        }
                    }
                    // SAFETY: as above.
                    unsafe {
                        set_status(cell, "");
                        refresh(cell);
                    }
                }
                Ok(ActivateOutcome::WrittenReloadFailed(reason)) => {
                    let text = translate_with(DEACTIVATE_RELOAD_FAILED, &[&reason]);
                    // SAFETY: as above.
                    unsafe {
                        set_status(cell, &text);
                        refresh(cell);
                    }
                }
                Err(reason) => {
                    cell.borrow_mut().form.restore_active(previous_active);
                    let text = translate_with(DEACTIVATE_FAILED, &[&reason]);
                    // SAFETY: as above.
                    unsafe {
                        set_status(cell, &text);
                        refresh(cell);
                    }
                }
            }
        };
        // SAFETY: the page is live and the signal emission holds a reference
        // to it for all of this call.
        unsafe { dispatch::spawn_for(page.cast(), work, finish) };
    });
}

/// Renames the selected script at once instead of staging it for the
/// account editor's OK (item 180(a)): asks for the new name in the same
/// modal dialog the staged rename used, then writes the account's current
/// plan on a worker thread and reloads the list from the server, the same
/// round trip [`activate_now`] uses. The script list and buttons stay
/// insensitive meanwhile. A write the server refuses restores the previous
/// name; nothing else staged is touched either way.
///
/// # Safety
///
/// `page` must be the live page the Rename button belongs to.
unsafe fn rename_now(page: gpointer) {
    guard("JmapFiltersPage::rename", (), || {
        // SAFETY: the page is live, per the contract. The hold keeps the
        // page and its state alive through the dialog below.
        let hold = unsafe { Hold::new(page.cast()) };
        // SAFETY: as above.
        let Some(cell) = (unsafe { state(page.cast()) }) else {
            return;
        };
        // SAFETY: the widgets are live, and no borrow is held.
        unsafe { flush(cell) };
        let Some(row) = cell.borrow().form.selected() else {
            return refuse(cell, NO_SCRIPT_SELECTED);
        };
        let previous_name = cell
            .borrow()
            .form
            .entries()
            .get(row)
            .map(|entry| entry.name.clone())
            .unwrap_or_default();

        // The name dialog runs a nested main loop, so it must run with no
        // borrow of the state held: the handlers it lets through take their
        // own.
        // SAFETY: the page is live, per the contract.
        let typed = unsafe { ask_for_name(page.cast(), RENAME_TITLE, &previous_name) };
        // The shell can destroy the page while the dialog runs. Its children
        // are then gone.
        if hold.disposed() {
            return;
        }
        let Some(name) = typed else {
            return;
        };
        if let Err(reason) = cell.borrow_mut().form.rename(row, &name) {
            return refuse(cell, reason);
        }

        let planned = {
            let ui = cell.borrow();
            (ui.link.clone(), ui.form.plan())
        };
        let (link, plan) = match planned {
            // The rename above always makes the form dirty, so a `None`
            // plan or link is not expected; leave nothing written either
            // way.
            (_, Ok(None)) | (None, Ok(Some(_))) => {
                cell.borrow_mut().form.restore_name(row, previous_name);
                // SAFETY: as above.
                unsafe { refresh(cell) };
                return;
            }
            (_, Err(reason)) => {
                cell.borrow_mut().form.restore_name(row, previous_name);
                let text = translate_with(RENAME_FAILED, &[&reason]);
                // SAFETY: as above.
                unsafe {
                    set_status(cell, &text);
                    refresh(cell);
                }
                return;
            }
            (Some(link), Ok(Some(plan))) => (link, plan),
        };

        // SAFETY: the widgets are live, and no borrow is held.
        unsafe {
            set_script_controls_sensitive(cell, false);
            set_status(cell, &translate(RENAMING));
        }

        let work = move || io::activate_now(&link, &plan);
        let finish = move |page: *mut GObject, outcome: Result<ActivateOutcome, String>| {
            // SAFETY: `with_strong` gives the live page.
            let Some(cell) = (unsafe { state(page) }) else {
                return;
            };
            match outcome {
                Ok(ActivateOutcome::Reloaded(loaded)) => {
                    {
                        let mut ui = cell.borrow_mut();
                        ui.form = FiltersForm::from_loaded(loaded);
                        let found = ui.form.entries().iter().position(|e| e.name == name);
                        ui.form.select(found);
                    }
                    // SAFETY: as above.
                    unsafe {
                        set_status(cell, "");
                        refresh(cell);
                    }
                }
                Ok(ActivateOutcome::WrittenReloadFailed(reason)) => {
                    let text = translate_with(RENAME_RELOAD_FAILED, &[&reason]);
                    // SAFETY: as above.
                    unsafe {
                        set_status(cell, &text);
                        refresh(cell);
                    }
                }
                Err(reason) => {
                    cell.borrow_mut().form.restore_name(row, previous_name);
                    let text = translate_with(RENAME_FAILED, &[&reason]);
                    // SAFETY: as above.
                    unsafe {
                        set_status(cell, &text);
                        refresh(cell);
                    }
                }
            }
        };
        // SAFETY: the page is live and the signal emission holds a reference
        // to it for all of this call.
        unsafe { dispatch::spawn_for(page.cast(), work, finish) };
    });
}

/// Adds a new script at once instead of staging it for the account
/// editor's OK (item 180(a)): asks for the name in the same modal dialog
/// the staged add used, then writes the account's current plan on a
/// worker thread and reloads the list from the server, the same round
/// trip [`rename_now`] uses. The script list and buttons stay insensitive
/// meanwhile. A write the server refuses removes the added script again;
/// nothing else staged is touched either way.
///
/// # Safety
///
/// `page` must be the live page the Add button belongs to.
unsafe fn add_now(page: gpointer) {
    guard("JmapFiltersPage::add", (), || {
        // SAFETY: the page is live, per the contract. The hold keeps the
        // page and its state alive through the dialog below.
        let hold = unsafe { Hold::new(page.cast()) };
        // SAFETY: as above.
        let Some(cell) = (unsafe { state(page.cast()) }) else {
            return;
        };
        // SAFETY: the widgets are live, and no borrow is held.
        unsafe { flush(cell) };

        // Suggests the managed name so that naming a script this opens it
        // straight into rules mode (`FiltersForm::add`), unless a script
        // already has that name.
        let unused = cell
            .borrow()
            .form
            .entries()
            .iter()
            .all(|entry| entry.name != MANAGED_SCRIPT_NAME);
        let suggested = if unused { MANAGED_SCRIPT_NAME } else { "" };

        // The name dialog runs a nested main loop, so it must run with no
        // borrow of the state held: the handlers it lets through take their
        // own.
        // SAFETY: the page is live, per the contract.
        let typed = unsafe { ask_for_name(page.cast(), ADD_TITLE, suggested) };
        // The shell can destroy the page while the dialog runs. Its children
        // are then gone.
        if hold.disposed() {
            return;
        }
        let Some(name) = typed else {
            return;
        };
        let row = match cell.borrow_mut().form.add(&name) {
            Ok(row) => row,
            Err(reason) => return refuse(cell, reason),
        };

        let planned = {
            let ui = cell.borrow();
            (ui.link.clone(), ui.form.plan())
        };
        let (link, plan) = match planned {
            // The add above always makes the form dirty, so a `None` plan
            // or link is not expected; leave nothing written either way.
            (_, Ok(None)) | (None, Ok(Some(_))) => {
                cell.borrow_mut().form.restore_added(row);
                // SAFETY: as above.
                unsafe { refresh(cell) };
                return;
            }
            (_, Err(reason)) => {
                cell.borrow_mut().form.restore_added(row);
                let text = translate_with(ADD_FAILED, &[&reason]);
                // SAFETY: as above.
                unsafe {
                    set_status(cell, &text);
                    refresh(cell);
                }
                return;
            }
            (Some(link), Ok(Some(plan))) => (link, plan),
        };

        // SAFETY: the widgets are live, and no borrow is held.
        unsafe {
            set_script_controls_sensitive(cell, false);
            set_status(cell, &translate(ADDING));
        }

        let work = move || io::activate_now(&link, &plan);
        let finish = move |page: *mut GObject, outcome: Result<ActivateOutcome, String>| {
            // SAFETY: `with_strong` gives the live page.
            let Some(cell) = (unsafe { state(page) }) else {
                return;
            };
            match outcome {
                Ok(ActivateOutcome::Reloaded(loaded)) => {
                    {
                        let mut ui = cell.borrow_mut();
                        ui.form = FiltersForm::from_loaded(loaded);
                        let found = ui.form.entries().iter().position(|e| e.name == name);
                        ui.form.select(found);
                    }
                    // SAFETY: as above.
                    unsafe {
                        set_status(cell, "");
                        refresh(cell);
                    }
                }
                Ok(ActivateOutcome::WrittenReloadFailed(reason)) => {
                    let text = translate_with(ADD_RELOAD_FAILED, &[&reason]);
                    // SAFETY: as above.
                    unsafe {
                        set_status(cell, &text);
                        refresh(cell);
                    }
                }
                Err(reason) => {
                    cell.borrow_mut().form.restore_added(row);
                    let text = translate_with(ADD_FAILED, &[&reason]);
                    // SAFETY: as above.
                    unsafe {
                        set_status(cell, &text);
                        refresh(cell);
                    }
                }
            }
        };
        // SAFETY: the page is live and the signal emission holds a reference
        // to it for all of this call.
        unsafe { dispatch::spawn_for(page.cast(), work, finish) };
    });
}

/// Removes the selected script at once instead of staging it for the
/// account editor's OK (item 180(a)): writes the account's current plan
/// on a worker thread and reloads the list from the server, the same
/// round trip [`rename_now`] uses. The script list and buttons stay
/// insensitive meanwhile. A write the server refuses puts the script back;
/// nothing else staged is touched either way.
///
/// # Safety
///
/// `page` must be the live page the Remove button belongs to.
unsafe fn remove_now(page: gpointer) {
    guard("JmapFiltersPage::remove", (), || {
        // SAFETY: the page is live, because its button emitted.
        let Some(cell) = (unsafe { state(page.cast()) }) else {
            return;
        };
        // SAFETY: the widgets are live, and no borrow is held.
        unsafe { flush(cell) };
        let Some(row) = cell.borrow().form.selected() else {
            return refuse(cell, NO_SCRIPT_SELECTED);
        };
        let Some(removed) = cell.borrow().form.entries().get(row).cloned() else {
            return refuse(cell, NO_SCRIPT_SELECTED);
        };
        if let Err(reason) = cell.borrow_mut().form.remove(row) {
            return refuse(cell, reason);
        }

        let planned = {
            let ui = cell.borrow();
            (ui.link.clone(), ui.form.plan())
        };
        let (link, plan) = match planned {
            // The removal above always makes the form dirty, so a `None`
            // plan or link is not expected; leave nothing written either
            // way.
            (_, Ok(None)) | (None, Ok(Some(_))) => {
                cell.borrow_mut().form.restore_removed(row, removed);
                // SAFETY: as above.
                unsafe { refresh(cell) };
                return;
            }
            (_, Err(reason)) => {
                cell.borrow_mut().form.restore_removed(row, removed);
                let text = translate_with(REMOVE_FAILED, &[&reason]);
                // SAFETY: as above.
                unsafe {
                    set_status(cell, &text);
                    refresh(cell);
                }
                return;
            }
            (Some(link), Ok(Some(plan))) => (link, plan),
        };

        // SAFETY: the widgets are live, and no borrow is held.
        unsafe {
            set_script_controls_sensitive(cell, false);
            set_status(cell, &translate(REMOVING));
        }

        let work = move || io::activate_now(&link, &plan);
        let finish = move |page: *mut GObject, outcome: Result<ActivateOutcome, String>| {
            // SAFETY: `with_strong` gives the live page.
            let Some(cell) = (unsafe { state(page) }) else {
                return;
            };
            match outcome {
                Ok(ActivateOutcome::Reloaded(loaded)) => {
                    {
                        let mut ui = cell.borrow_mut();
                        ui.form = FiltersForm::from_loaded(loaded);
                    }
                    // SAFETY: as above.
                    unsafe {
                        set_status(cell, "");
                        refresh(cell);
                    }
                }
                Ok(ActivateOutcome::WrittenReloadFailed(reason)) => {
                    let text = translate_with(REMOVE_RELOAD_FAILED, &[&reason]);
                    // SAFETY: as above.
                    unsafe {
                        set_status(cell, &text);
                        refresh(cell);
                    }
                }
                Err(reason) => {
                    cell.borrow_mut().form.restore_removed(row, removed);
                    let text = translate_with(REMOVE_FAILED, &[&reason]);
                    // SAFETY: as above.
                    unsafe {
                        set_status(cell, &text);
                        refresh(cell);
                    }
                }
            }
        };
        // SAFETY: the page is live and the signal emission holds a reference
        // to it for all of this call.
        unsafe { dispatch::spawn_for(page.cast(), work, finish) };
    });
}

/// What one rules-mode button press asks [`RulesEditor`] to do.
enum RuleAction {
    Add,
    Edit,
    Remove,
    Up,
    Down,
    Enable,
    Disable,
}

unsafe extern "C" fn on_rule_add(_button: *mut GObject, page: gpointer) {
    // SAFETY: the page is live, because its button emitted.
    unsafe { act_rule(page, RuleAction::Add) };
}

unsafe extern "C" fn on_rule_edit(_button: *mut GObject, page: gpointer) {
    // SAFETY: as above.
    unsafe { act_rule(page, RuleAction::Edit) };
}

unsafe extern "C" fn on_rule_remove(_button: *mut GObject, page: gpointer) {
    // SAFETY: as above.
    unsafe { act_rule(page, RuleAction::Remove) };
}

unsafe extern "C" fn on_rule_up(_button: *mut GObject, page: gpointer) {
    // SAFETY: as above.
    unsafe { act_rule(page, RuleAction::Up) };
}

unsafe extern "C" fn on_rule_down(_button: *mut GObject, page: gpointer) {
    // SAFETY: as above.
    unsafe { act_rule(page, RuleAction::Down) };
}

unsafe extern "C" fn on_rule_enable(_button: *mut GObject, page: gpointer) {
    // SAFETY: as above.
    unsafe { act_rule(page, RuleAction::Enable) };
}

unsafe extern "C" fn on_rule_disable(_button: *mut GObject, page: gpointer) {
    // SAFETY: as above.
    unsafe { act_rule(page, RuleAction::Disable) };
}

/// Runs one staged rules-mode action and shows the lists again. A refusal
/// goes to the status label; nothing reaches the server here.
///
/// # Safety
///
/// `page` must be the live page the button belongs to.
unsafe fn act_rule(page: gpointer, action: RuleAction) {
    guard("JmapFiltersPage::rule-action", (), || {
        // SAFETY: the page is live, per the contract. The hold keeps the
        // page and its state alive through the dialog below.
        let hold = unsafe { Hold::new(page.cast()) };
        // SAFETY: as above.
        let Some(cell) = (unsafe { state(page.cast()) }) else {
            return;
        };
        if cell.borrow().rules.is_none() {
            return;
        }
        let selected = cell.borrow().rules.as_ref().and_then(RulesEditor::selected);

        // The rule dialog runs a nested main loop, so it must run with no
        // borrow of the state held.
        let changed = match action {
            RuleAction::Add => {
                let (folders, extensions) = {
                    let ui = cell.borrow();
                    (ui.form.folders().to_vec(), ui.form.extensions().to_vec())
                };
                let draft = RuleDraft::new();
                // SAFETY: the page is live, per the contract.
                match unsafe {
                    rule_dialog::edit_rule(
                        page.cast(),
                        ADD_RULE_TITLE,
                        &draft,
                        &folders,
                        &extensions,
                    )
                } {
                    None => Ok(false),
                    Some(rule) => {
                        cell.borrow_mut()
                            .rules
                            .as_mut()
                            .expect("checked above")
                            .add(rule);
                        Ok(true)
                    }
                }
            }
            RuleAction::Edit => {
                let Some(index) = selected else {
                    return refuse(cell, NO_RULE_SELECTED);
                };
                let (folders, extensions, draft) = {
                    let ui = cell.borrow();
                    let Some(rule) = ui.rules.as_ref().and_then(|rules| rules.rules().get(index))
                    else {
                        return refuse(cell, NO_RULE_SELECTED);
                    };
                    (
                        ui.form.folders().to_vec(),
                        ui.form.extensions().to_vec(),
                        RuleDraft::from_rule(rule),
                    )
                };
                // SAFETY: as above.
                match unsafe {
                    rule_dialog::edit_rule(
                        page.cast(),
                        EDIT_RULE_TITLE,
                        &draft,
                        &folders,
                        &extensions,
                    )
                } {
                    None => Ok(false),
                    Some(rule) => cell
                        .borrow_mut()
                        .rules
                        .as_mut()
                        .expect("checked above")
                        .replace(index, rule)
                        .map(|()| true),
                }
            }
            RuleAction::Remove => match selected {
                None => return refuse(cell, NO_RULE_SELECTED),
                Some(index) => cell
                    .borrow_mut()
                    .rules
                    .as_mut()
                    .expect("checked above")
                    .remove(index)
                    .map(|()| true),
            },
            RuleAction::Up => match selected {
                None => return refuse(cell, NO_RULE_SELECTED),
                Some(index) => cell
                    .borrow_mut()
                    .rules
                    .as_mut()
                    .expect("checked above")
                    .move_up(index)
                    .map(|()| true),
            },
            RuleAction::Down => match selected {
                None => return refuse(cell, NO_RULE_SELECTED),
                Some(index) => cell
                    .borrow_mut()
                    .rules
                    .as_mut()
                    .expect("checked above")
                    .move_down(index)
                    .map(|()| true),
            },
            RuleAction::Enable | RuleAction::Disable => match selected {
                None => return refuse(cell, NO_RULE_SELECTED),
                Some(index) => cell
                    .borrow_mut()
                    .rules
                    .as_mut()
                    .expect("checked above")
                    .toggle_enabled(index)
                    .map(|()| true),
            },
        };
        // As in `act`.
        if hold.disposed() {
            return;
        }

        match changed {
            Ok(false) => {}
            Ok(true) => {
                // SAFETY: the widgets are live, and no borrow is held.
                unsafe { write_back_rules(cell) };
            }
            Err(reason) => refuse(cell, reason),
        }
    });
}

/// Regenerates the managed script's text from the staged rules (keeping the
/// round trip [`script::load`] relies on), writes it into the form, and
/// redraws every list.
///
/// # Safety
///
/// As [`refresh`].
unsafe fn write_back_rules(cell: &RefCell<UiState>) {
    let Some(row) = cell.borrow().showing else {
        return;
    };
    let generated = {
        let ui = cell.borrow();
        let Some(rules) = &ui.rules else {
            return;
        };
        script::generate_script(rules.rules(), ui.form.has_mailboxid())
    };
    let _ = cell.borrow_mut().form.set_text(row, &generated);
    // SAFETY: the widgets are live, and no borrow is held.
    unsafe {
        set_status(cell, "");
        refresh(cell);
    }
}

/// Shows why the page did not do what the button asked.
fn refuse(cell: &RefCell<UiState>, reason: &CStr) {
    // SAFETY: the status label is a live child of the page, and no borrow is
    // held across the call.
    unsafe { set_status(cell, &translate(reason)) };
}

/// Builds the name dialog, unshown and not yet run. Returns the dialog and
/// its name entry.
///
/// Split out from [`ask_for_name`] so a test can check the dialog's
/// keyboard behaviour without running its nested main loop.
///
/// # Safety
///
/// `page` must be the live page, on the main thread.
unsafe fn build_name_dialog(
    page: *mut GObject,
    title: &'static CStr,
    initial: &str,
) -> (*mut GtkWidget, *mut GtkWidget) {
    // SAFETY: the page is live, per the contract. Every widget is new, and
    // the caller owns the returned dialog.
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
        // The OK button answers Enter too, as GNOME HIG expects of a
        // single-field dialog: a default response for the dialog, and the
        // entry forwards Enter to it.
        gtk_dialog_set_default_response(dialog.cast(), GTK_RESPONSE_OK);

        let row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        let label = gtk_label_new_with_mnemonic(translate_static(NAME_LABEL));
        let entry = gtk_entry_new();
        set_text(entry.cast(), initial);
        gtk_entry_set_activates_default(entry.cast(), GTRUE);
        gtk_label_set_mnemonic_widget(label.cast(), entry);
        gtk_box_pack_start(row.cast(), label, GFALSE, GFALSE, 0);
        gtk_widget_set_hexpand(entry, GTRUE);
        gtk_box_pack_start(row.cast(), entry, GTRUE, GTRUE, 0);
        let content_area = gtk_dialog_get_content_area(dialog.cast());
        // GNOME HIG: 12px between a dialog's content and its edges.
        gtk_container_set_border_width(content_area.cast(), 12);
        gtk_container_add(content_area.cast(), row);

        (dialog, entry)
    }
}

/// Asks for a script name in a modal dialog. `None` means the user
/// cancelled.
///
/// # Safety
///
/// `page` must be the live page, on the main thread.
unsafe fn ask_for_name(page: *mut GObject, title: &'static CStr, initial: &str) -> Option<String> {
    // SAFETY: the page is live, per the contract. The dialog owns its
    // entry, and this function destroys the dialog.
    unsafe {
        let (dialog, entry) = build_name_dialog(page, title, initial);
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
        let generation = cell.borrow_mut().begin_check();
        // SAFETY: the widgets are live, and no borrow is held.
        unsafe { set_status(cell, &translate(CHECKING)) };

        let work = move || io::check(&job.link, &job.text);
        let finish = move |page: *mut GObject, answer: Result<Option<String>, String>| {
            // SAFETY: `with_strong` gives the live page.
            let Some(cell) = (unsafe { state(page) }) else {
                return;
            };
            // A newer Check started after this one: its answer is stale.
            if !cell.borrow().check_is_current(generation) {
                return;
            }
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

#[cfg(test)]
mod tests {
    use std::os::raw::c_char;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use evo_sys::{
        gtk_container_get_border_width, gtk_label_get_text, gtk_source_buffer_get_language,
        gtk_source_view_get_auto_indent, gtk_text_buffer_new, gtk_text_view_get_accepts_tab,
        gtk_tree_view_get_column, gtk_window_get_default_widget,
    };
    use glib_sys::{G_SOURCE_REMOVE, GList, g_idle_add, g_list_free, g_main_context_iteration};
    use gobject_sys::{g_object_get, g_object_new_with_properties, g_object_set_data_full};
    use jmap_backend_core::marshal::read_string;
    use jmap_backend_core::subclass::register_static;
    use jmap_tracing_capture::{capture, has};
    use tracing::Level;

    use super::*;

    unsafe extern "C" {
        fn gtk_init_check(argc: *mut c_int, argv: *mut *mut *mut c_char) -> gboolean;
        fn gtk_window_new(kind: c_int) -> *mut GtkWidget;
        fn gtk_window_list_toplevels() -> *mut GList;
        fn gtk_dialog_response(dialog: *mut GtkWidget, response: c_int);
    }

    static EVENTS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

    fn note(event: &'static str) {
        EVENTS.lock().unwrap().push(event);
    }

    unsafe extern "C" fn page_finalized(_data: gpointer) {
        note("page finalized");
    }

    unsafe extern "C" fn close_window(window: gpointer) -> gboolean {
        // SAFETY: the test window is live until this call destroys it.
        unsafe { gtk_widget_destroy(window.cast()) };
        note("window destroyed");
        G_SOURCE_REMOVE
    }

    unsafe extern "C" fn answer_ok(_data: gpointer) -> gboolean {
        // SAFETY: the list and its windows are live on this thread. The
        // list is freed below; GTK owns the windows.
        unsafe {
            let dialog_type = g_type_from_name(c"GtkDialog".as_ptr());
            let list = gtk_window_list_toplevels();
            let mut node = list;
            while !node.is_null() {
                let window = (*node).data;
                if g_type_check_instance_is_a(window.cast(), dialog_type) != GFALSE {
                    note("dialog answered");
                    gtk_dialog_response(window.cast(), GTK_RESPONSE_OK);
                }
                node = (*node).next;
            }
            g_list_free(list);
        }
        G_SOURCE_REMOVE
    }

    /// Types a new name into the name dialog [`rename_now`] opened, then
    /// answers OK, the way [`answer_ok`] answers a dialog whose initial
    /// text needs no change. The entry is the dialog content area's one
    /// row's second child, the layout [`build_name_dialog`] builds.
    unsafe extern "C" fn answer_rename(_data: gpointer) -> gboolean {
        // SAFETY: the dialog `rename_now` opened is live on this thread,
        // inside the nested main loop this idle runs in. Every list is
        // freed once walked; GTK owns the windows and their children.
        unsafe {
            let dialog_type = g_type_from_name(c"GtkDialog".as_ptr());
            let list = gtk_window_list_toplevels();
            let mut node = list;
            while !node.is_null() {
                let window = (*node).data;
                if g_type_check_instance_is_a(window.cast(), dialog_type) != GFALSE {
                    let content = gtk_dialog_get_content_area(window.cast());
                    let rows = gtk_container_get_children(content.cast());
                    if !rows.is_null() {
                        let fields = gtk_container_get_children((*rows).data.cast());
                        if !fields.is_null() && !(*fields).next.is_null() {
                            set_text((*(*fields).next).data.cast(), "Renamed");
                        }
                        g_list_free(fields);
                    }
                    g_list_free(rows);
                    gtk_dialog_response(window.cast(), GTK_RESPONSE_OK);
                }
                node = (*node).next;
            }
            g_list_free(list);
        }
        G_SOURCE_REMOVE
    }

    /// The mnemonic underscore marks one letter per label. Panics if a
    /// label has none, which would be a test bug, not a product one.
    fn mnemonic(label: &CStr) -> char {
        let text = label.to_str().unwrap();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '_' {
                return chars.next().unwrap().to_ascii_lowercase();
            }
        }
        panic!("no mnemonic in {text:?}");
    }

    /// The rules list's "_Edit" button and the "_Edit as text" checkbox can
    /// show at the same time: `want_rules` (which shows the rules box) only
    /// holds when `is_managed` (which shows the checkbox) also holds. Two
    /// controls in the same window cannot share a mnemonic letter, or
    /// Alt+E opens only one of them and the other needs a mouse.
    #[test]
    fn rule_edit_and_edit_as_text_keep_different_mnemonics() {
        assert_ne!(mnemonic(RULE_EDIT_LABEL), mnemonic(EDIT_AS_TEXT_LABEL));
    }

    /// Item 180(b): the banner shows the first time the selection lands on a
    /// managed script that does not parse as rules.
    #[test]
    fn external_edit_note_shows_for_a_freshly_selected_unparseable_managed_script() {
        assert!(show_external_edit_note(true, true, false, false));
    }

    /// Item 180(b): a later redraw of the same script (no new selection)
    /// leaves the status label alone, so the banner is not forced again.
    #[test]
    fn external_edit_note_does_not_show_on_a_later_redraw_of_the_same_script() {
        assert!(!show_external_edit_note(false, true, false, false));
    }

    /// Item 180(b): a script that parses as rules is not "changed outside
    /// Evolution".
    #[test]
    fn external_edit_note_does_not_show_for_a_parseable_script() {
        assert!(!show_external_edit_note(true, true, true, false));
    }

    /// Item 180(b): the note is for the managed script only.
    #[test]
    fn external_edit_note_does_not_show_for_an_unmanaged_script() {
        assert!(!show_external_edit_note(true, false, false, false));
    }

    /// Item 180(b): a text edit made in this page is not an edit from
    /// outside Evolution, so it clears the note for good.
    #[test]
    fn external_edit_note_does_not_show_once_the_user_edited_this_script_as_text() {
        assert!(!show_external_edit_note(true, true, false, true));
    }

    /// "Deactivate" and the rules list's "Add" button can show at the same
    /// time: deactivate is sensitive whenever the selected script is active,
    /// and the rules box (which holds rule add) shows whenever the selected
    /// script is the managed one and parses, regardless of active state. An
    /// active, managed, parseable script shows both at once. Two controls
    /// in the same window cannot share a mnemonic letter, or Alt+D opens
    /// only one of them and the other needs a mouse.
    #[test]
    fn deactivate_and_rule_add_keep_different_mnemonics() {
        assert_ne!(mnemonic(DEACTIVATE_LABEL), mnemonic(RULE_ADD_LABEL));
    }

    /// "Activate" and the Account Editor dialog's own stock "Cancel" button
    /// are shown in the same window at the same time: the Server Filters
    /// page never hides the dialog's own button row. Found live by
    /// `ci/gui-scenario/scenarios/a11y-audit.py`'s `audit_mnemonics` check,
    /// both bound Alt+C. GNOME's stock Cancel button mnemonic is always
    /// "c" (every Evolution dialog agrees on it; not ours to change), so
    /// this checks our own label directly rather than through a stock
    /// GTK dialog built just for the test.
    #[test]
    fn activate_does_not_take_the_dialogs_own_cancel_mnemonic() {
        assert_ne!(mnemonic(ACTIVATE_LABEL), 'c');
    }

    /// GNOME HIG: a button that opens a dialog carries a trailing ellipsis,
    /// like `send_later`'s "_Custom Time…" already does. "_Add" and
    /// "_Rename" open the New/Rename Filter Script dialog; the rules list's
    /// "A_dd" and "Edi_t" open the New/Edit Rule dialog.
    #[test]
    fn dialog_opening_buttons_show_an_ellipsis() {
        for label in [ADD_LABEL, RENAME_LABEL, RULE_ADD_LABEL, RULE_EDIT_LABEL] {
            assert!(label.to_str().unwrap().ends_with('…'), "{label:?}");
        }
    }

    /// GNOME HIG: every flow works with the keyboard only. The name dialog
    /// has one field, so Enter after typing it should confirm, the same as
    /// clicking OK. Needs a display; skips without one.
    #[test]
    fn the_name_dialog_lets_enter_confirm_it() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        // SAFETY: a plain toplevel window stands in for the live page;
        // `build_name_dialog` only passes it to `gtk_widget_get_toplevel`.
        // Both widgets are destroyed before returning.
        unsafe {
            let window = gtk_window_new(0);
            let (dialog, entry) = build_name_dialog(window.cast(), c"Test", "x");

            assert!(
                !gtk_window_get_default_widget(dialog.cast()).is_null(),
                "the dialog has no default response: Enter would do nothing"
            );
            let mut activates_default: gboolean = GFALSE;
            g_object_get(
                entry.cast(),
                c"activates-default".as_ptr(),
                &mut activates_default as *mut gboolean,
                ptr::null::<c_char>(),
            );
            assert_ne!(
                activates_default, GFALSE,
                "the name entry does not activate the default: Enter would do nothing"
            );

            gtk_widget_destroy(dialog);
            gtk_widget_destroy(window);
        }
    }

    /// Item 171a: the text-mode editor is a GtkSourceView, and its buffer
    /// carries the "sieve" language once `sieve.lang` is installed where
    /// GtkSourceView looks for it (`cmake/Backends.cmake`'s install rule).
    /// Needs a display; skips without one.
    #[test]
    fn the_script_view_gets_the_sieve_language() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        // SAFETY: `gtk_source_view_new` returns a live, unowned widget; its
        // buffer is borrowed from it and outlives this call. Both are
        // destroyed before returning.
        unsafe {
            let view = gtk_source_view_new();
            let buffer = gtk_text_view_get_buffer(view.cast());
            set_sieve_language(buffer.cast());
            assert!(
                !gtk_source_buffer_get_language(buffer.cast()).is_null(),
                "the script view's buffer has no \"sieve\" language; is \
                 sieve.lang installed in GtkSourceView's search path?"
            );
            gtk_widget_destroy(view);
        }
    }

    /// Item 180(e): with no completion suggestion showing, Tab must move
    /// focus to the next field, not insert a tab character; auto-indent
    /// takes over the indenting work Tab no longer does. Needs a display;
    /// skips without one.
    #[test]
    fn the_script_view_lets_tab_move_focus_instead_of_indenting() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        let gtype = register_static::<FiltersPage>();
        // SAFETY: the type is registered; `instance_init` runs `build_widgets`.
        unsafe {
            let page = g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null());
            let source_view = state(page).unwrap().borrow().source_view;

            assert_eq!(
                gtk_text_view_get_accepts_tab(source_view.cast()),
                GFALSE,
                "Tab inserts a tab character instead of moving focus"
            );
            assert_eq!(
                gtk_source_view_get_auto_indent(source_view.cast()),
                GTRUE,
                "auto-indent is off, so Tab was the only way to indent"
            );

            gtk_widget_destroy(page.cast());
        }
    }

    /// A fresh buffer holding `text`, cursor at its very end. `GtkTextBuffer`
    /// is a plain GObject, not a widget (as `tests/text_iter.rs` in evo-sys
    /// notes of its own equivalent helper), so this needs no display: it is
    /// the one test below that is not display-gated.
    fn buffer_ending_with(text: &str) -> (*mut GtkTextBuffer, GtkTextIter) {
        // SAFETY: a fresh buffer and a fresh iter, both local to the caller.
        unsafe {
            let buffer = gtk_text_buffer_new(ptr::null_mut());
            let mut start = GtkTextIter::default();
            gtk_text_buffer_get_start_iter(buffer, &mut start);
            let c_text = cstring_lossy(text);
            gtk_text_buffer_insert(buffer, &mut start, c_text.as_ptr(), -1);
            let mut end = GtkTextIter::default();
            gtk_text_buffer_get_end_iter(buffer, &mut end);
            (buffer, end)
        }
    }

    /// Item 171c's own core: replacing the word the cursor sits at the end
    /// of with the chosen candidate, leaving everything before it alone.
    #[test]
    fn replace_word_at_swaps_only_the_trailing_word() {
        // SAFETY: a fresh, private buffer; destroyed before returning.
        unsafe {
            let (buffer, end) = buffer_ending_with("if header :con");
            replace_word_at(buffer, &end, ":contains");
            let mut start = GtkTextIter::default();
            let mut new_end = GtkTextIter::default();
            gtk_text_buffer_get_start_iter(buffer, &mut start);
            gtk_text_buffer_get_end_iter(buffer, &mut new_end);
            let result = take_string(gtk_text_buffer_get_text(buffer, &start, &new_end, GFALSE))
                .unwrap_or_default();
            assert_eq!(result, "if header :contains");
            g_object_unref(buffer.cast());
        }
    }

    /// A word right after whitespace, with nothing to replace yet: the
    /// candidate is inserted, nothing before the cursor is touched.
    #[test]
    fn replace_word_at_inserts_after_whitespace_with_no_partial_word() {
        // SAFETY: as above.
        unsafe {
            let (buffer, end) = buffer_ending_with("if ");
            replace_word_at(buffer, &end, "header");
            let mut start = GtkTextIter::default();
            let mut new_end = GtkTextIter::default();
            gtk_text_buffer_get_start_iter(buffer, &mut start);
            gtk_text_buffer_get_end_iter(buffer, &mut new_end);
            let result = take_string(gtk_text_buffer_get_text(buffer, &start, &new_end, GFALSE))
                .unwrap_or_default();
            assert_eq!(result, "if header");
            g_object_unref(buffer.cast());
        }
    }

    /// Item 171c: Ctrl+Space opens a matching list, Up/Down moves the
    /// selection, and accepting a candidate replaces the word under the
    /// cursor and closes the popup again. Builds a live page, unlike the
    /// two tests above, because the popup's own widgets live on the
    /// page's `UiState`, not on a bare buffer. "e" matches three base
    /// Sieve candidates ("elsif", "else", "exists"), RFC 5228 syntax a
    /// freshly built page's default (extension-less) form already
    /// offers, so Up/Down has more than one row to move between. Needs a
    /// display; skips without one.
    #[test]
    fn completion_opens_navigates_and_accepts_a_candidate() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        let gtype = register_static::<FiltersPage>();
        // SAFETY: the type is registered; the window sinks the floating
        // page and owns its only reference, as the account editor does.
        // Both are destroyed before returning.
        unsafe {
            let window = gtk_window_new(0);
            let page = g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null());
            gtk_container_add(window.cast(), page.cast());
            gtk_widget_show_all(window);

            let cell = state(page.cast()).expect("a freshly built page has state");
            let buffer = cell.borrow().buffer;
            let text = cstring_lossy("e");
            let mut start = GtkTextIter::default();
            gtk_text_buffer_get_start_iter(buffer, &mut start);
            gtk_text_buffer_insert(buffer, &mut start, text.as_ptr(), -1);
            let mut end = GtkTextIter::default();
            gtk_text_buffer_get_end_iter(buffer, &mut end);
            gtk_text_buffer_place_cursor(buffer, &end);

            open_completion(cell);
            {
                let ui = cell.borrow();
                let texts: Vec<&str> = ui.completion_candidates.iter().map(|c| c.text).collect();
                assert_eq!(
                    texts,
                    vec!["elsif", "else", "exists"],
                    "\"e\" should match these three base candidates, in table order"
                );
            }

            move_completion_selection(cell, 1);
            assert_eq!(
                cell.borrow().completion_selected,
                1,
                "Down did not move the selection to the second candidate"
            );
            move_completion_selection(cell, -1);
            assert_eq!(
                cell.borrow().completion_selected,
                0,
                "Up did not move the selection back to the first candidate"
            );

            accept_completion(cell);
            assert!(
                cell.borrow().completion_candidates.is_empty(),
                "accepting a candidate did not close the popup"
            );
            let result = get_text(buffer.cast());
            assert_eq!(
                result, "elsif",
                "accept did not replace the partial word with the selected candidate"
            );

            gtk_widget_destroy(window);
        }
    }

    /// Item 180(e): while the popup is open, Right arrow accepts the
    /// selected candidate the same way Tab and Enter do. No display
    /// needed: `completion_key_action` is a pure function.
    #[test]
    fn right_arrow_accepts_like_tab_and_enter_while_the_popup_is_open() {
        assert_eq!(
            completion_key_action(GDK_KEY_Right, 0, true),
            CompletionKey::Accept
        );
        assert_eq!(
            completion_key_action(GDK_KEY_Tab, 0, true),
            CompletionKey::Accept
        );
        assert_eq!(
            completion_key_action(GDK_KEY_Return, 0, true),
            CompletionKey::Accept
        );
    }

    /// Item 180(e): with the popup closed, Right arrow is not a completion
    /// key, so the buffer moves the cursor as usual.
    #[test]
    fn right_arrow_with_no_popup_is_left_to_the_buffer() {
        assert_eq!(
            completion_key_action(GDK_KEY_Right, 0, false),
            CompletionKey::Ignore
        );
    }

    /// Iterates the default main context, blocking between rounds, until
    /// `done` is true or `timeout` elapses. The validate debounce (180(e)'s
    /// typing-triggered completion included) only fires on a real
    /// `g_timeout_add` source, so a test that waits for it must pump the
    /// loop itself; nothing else in this process does.
    fn pump_until(done: impl Fn() -> bool, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while !done() {
            assert!(
                Instant::now() < deadline,
                "main loop never fired the debounce"
            );
            // SAFETY: the default context; this test owns the thread.
            unsafe { g_main_context_iteration(ptr::null_mut(), GTRUE) };
        }
    }

    /// Item 180(e): typing two or more characters of a matching word opens
    /// the completion popup on its own, once the existing validate debounce
    /// fires, with no Ctrl+Space needed. Needs a display; skips without one.
    #[test]
    fn typing_two_characters_of_a_match_opens_completion_after_the_debounce() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        let gtype = register_static::<FiltersPage>();
        // SAFETY: the type is registered; the window sinks the floating
        // page and owns its only reference, as the account editor does.
        unsafe {
            let window = gtk_window_new(0);
            let page = g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null());
            gtk_container_add(window.cast(), page.cast());
            gtk_widget_show_all(window);

            let cell = state(page.cast()).expect("a freshly built page has state");
            let buffer = cell.borrow().buffer;
            let text = cstring_lossy("el");
            let mut start = GtkTextIter::default();
            gtk_text_buffer_get_start_iter(buffer, &mut start);
            gtk_text_buffer_insert(buffer, &mut start, text.as_ptr(), -1);
            let mut end = GtkTextIter::default();
            gtk_text_buffer_get_end_iter(buffer, &mut end);
            gtk_text_buffer_place_cursor(buffer, &end);

            pump_until(
                || !cell.borrow().completion_candidates.is_empty(),
                Duration::from_secs(5),
            );
            let texts: Vec<&str> = cell
                .borrow()
                .completion_candidates
                .iter()
                .map(|c| c.text)
                .collect();
            assert_eq!(
                texts,
                vec!["elsif", "else", "exists"],
                "typing \"el\" should auto-open the same matches an explicit trigger would"
            );

            gtk_widget_destroy(window);
        }
    }

    /// Item 180(e): typing a single character never auto-opens the popup
    /// (the "2+ characters" gate), even once the debounce fires. Needs a
    /// display; skips without one.
    #[test]
    fn typing_one_character_does_not_auto_open_completion() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        let gtype = register_static::<FiltersPage>();
        // SAFETY: as the test above.
        unsafe {
            let window = gtk_window_new(0);
            let page = g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null());
            gtk_container_add(window.cast(), page.cast());
            gtk_widget_show_all(window);

            let cell = state(page.cast()).expect("a freshly built page has state");
            let buffer = cell.borrow().buffer;
            let text = cstring_lossy("e");
            let mut start = GtkTextIter::default();
            gtk_text_buffer_get_start_iter(buffer, &mut start);
            gtk_text_buffer_insert(buffer, &mut start, text.as_ptr(), -1);
            let mut end = GtkTextIter::default();
            gtk_text_buffer_get_end_iter(buffer, &mut end);
            gtk_text_buffer_place_cursor(buffer, &end);

            // Wait past the debounce with no predicate to reach: there is
            // nothing to pump_until for here, so iterate a fixed stretch.
            // SAFETY: the default context; this test owns the thread.
            let deadline = Instant::now() + Duration::from_millis(VALIDATE_DEBOUNCE_MS as u64 * 3);
            while Instant::now() < deadline {
                g_main_context_iteration(ptr::null_mut(), GFALSE);
            }
            assert!(
                cell.borrow().completion_candidates.is_empty(),
                "a single typed character must not auto-open the popup"
            );

            gtk_widget_destroy(window);
        }
    }

    /// Item 182(c): the completion popup's open/close/accept states are
    /// invisible to AT-SPI, so they trace instead. Needs a display; skips
    /// without one.
    #[test]
    fn completion_open_close_and_accept_trace_their_state() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        let gtype = register_static::<FiltersPage>();
        // SAFETY: as `completion_opens_navigates_and_accepts_a_candidate`.
        unsafe {
            let window = gtk_window_new(0);
            let page = g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null());
            gtk_container_add(window.cast(), page.cast());
            gtk_widget_show_all(window);

            let cell = state(page.cast()).expect("a freshly built page has state");
            let buffer = cell.borrow().buffer;
            let text = cstring_lossy("e");
            let mut start = GtkTextIter::default();
            gtk_text_buffer_get_start_iter(buffer, &mut start);
            gtk_text_buffer_insert(buffer, &mut start, text.as_ptr(), -1);
            let mut end = GtkTextIter::default();
            gtk_text_buffer_get_end_iter(buffer, &mut end);
            gtk_text_buffer_place_cursor(buffer, &end);

            let (_, captured) = capture(|| {
                open_completion(cell);
                accept_completion(cell);
            });

            assert!(
                has(&captured, Level::TRACE, "word", "e"),
                "expected a traced word=\"e\" on open, got {captured:?}"
            );
            assert!(
                has(&captured, Level::TRACE, "candidate_count", "3"),
                "expected a traced candidate_count=3 on open, got {captured:?}"
            );
            assert!(
                has(&captured, Level::TRACE, "accepted", "elsif"),
                "expected a traced accepted=\"elsif\" on accept, got {captured:?}"
            );

            gtk_widget_destroy(window);
        }
    }

    /// GNOME HIG: 12px between a dialog's content and its edges. Needs a
    /// display; skips without one.
    #[test]
    fn the_name_dialog_has_a_hig_content_margin() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        // SAFETY: a plain toplevel window stands in for the live page;
        // `build_name_dialog` only passes it to `gtk_widget_get_toplevel`.
        // Both widgets are destroyed before returning.
        unsafe {
            let window = gtk_window_new(0);
            let (dialog, _entry) = build_name_dialog(window.cast(), c"Test", "x");

            let content_area = gtk_dialog_get_content_area(dialog.cast());
            assert_eq!(
                gtk_container_get_border_width(content_area.cast()),
                12,
                "the content area has no HIG margin: content touches the dialog's edges"
            );

            gtk_widget_destroy(dialog);
            gtk_widget_destroy(window);
        }
    }

    /// A fresh [`UiState`], not tied to any real widget.
    fn fresh_state() -> UiState {
        UiState {
            link: None,
            form: FiltersForm::default(),
            showing: None,
            rules: None,
            force_text: false,
            syncing: false,
            store: ptr::null_mut(),
            view: ptr::null_mut(),
            source_label: ptr::null_mut(),
            source_view: ptr::null_mut(),
            buffer: ptr::null_mut(),
            status: ptr::null_mut(),
            add: ptr::null_mut(),
            rename: ptr::null_mut(),
            remove: ptr::null_mut(),
            activate: ptr::null_mut(),
            deactivate: ptr::null_mut(),
            check: ptr::null_mut(),
            edit_as_text: ptr::null_mut(),
            rules_box: ptr::null_mut(),
            rules_store: ptr::null_mut(),
            rules_view: ptr::null_mut(),
            rule_add: ptr::null_mut(),
            rule_edit: ptr::null_mut(),
            rule_remove: ptr::null_mut(),
            rule_up: ptr::null_mut(),
            rule_down: ptr::null_mut(),
            rule_enable: ptr::null_mut(),
            rule_disable: ptr::null_mut(),
            check_generation: 0,
            error_tag: ptr::null_mut(),
            validate_timeout: 0,
            saving_text: false,
            completion_window: ptr::null_mut(),
            completion_list: ptr::null_mut(),
            completion_candidates: Vec::new(),
            completion_selected: 0,
            offline: false,
            shell: ptr::null_mut(),
            shell_online_handler: 0,
            shell_network_handler: 0,
        }
    }

    /// Two Check clicks before the first answers: the first click's reply
    /// must not overwrite the second click's, once it lands later.
    #[test]
    fn a_stale_check_answer_is_not_current() {
        let mut state = fresh_state();

        let first = state.begin_check();
        let second = state.begin_check();

        assert!(
            !state.check_is_current(first),
            "a stale Check answer looked current"
        );
        assert!(
            state.check_is_current(second),
            "the latest Check answer was rejected"
        );
    }

    /// The shell destroys the account editor while the name dialog of Add
    /// runs. The page must stay alive until the action ends, and the action
    /// must not continue on the destroyed page. Needs a display; skips
    /// without one.
    #[test]
    fn destroying_the_page_during_the_name_dialog_keeps_it_until_the_action_ends() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        let gtype = register_static::<FiltersPage>();
        // SAFETY: the type is registered. The window sinks the floating page
        // and owns its only reference, as the account editor does.
        unsafe {
            let window = gtk_window_new(0);
            let page = g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null());
            // GObject frees qdata in finalize, not in dispose. A NULL value
            // would remove the key, so the probe stores a dummy one.
            g_object_set_data_full(page, c"probe".as_ptr(), page.cast(), Some(page_finalized));
            gtk_container_add(window.cast(), page.cast());
            gtk_widget_show_all(window);
            // Both run inside the nested main loop of the dialog, in order.
            g_idle_add(Some(close_window), window.cast());
            g_idle_add(Some(answer_ok), ptr::null_mut());
            add_now(page.cast());
        }
        note("action returned");
        assert_eq!(
            *EVENTS.lock().unwrap(),
            [
                "window destroyed",
                "dialog answered",
                "page finalized",
                "action returned"
            ]
        );
    }

    /// Layout (item 174 (a)): a long script or rule name must not force the
    /// page to scroll sideways. Without a fixed sizing mode, a tree view
    /// column's width request grows with its longest cell, and the page
    /// cannot scroll horizontally (`hscrollbar-policy` is `NEVER`), so the
    /// name column would otherwise widen the whole page past the window.
    /// Needs a display; skips without one.
    #[test]
    fn a_name_column_does_not_grow_with_its_content() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        // SAFETY: a plain list store and tree view, both destroyed (the
        // store's one reference moves to the view) before returning.
        unsafe {
            let mut types = [G_TYPE_STRING];
            let store = gtk_list_store_newv(1, types.as_mut_ptr());
            let view = gtk_tree_view_new_with_model(store.cast::<GtkTreeModel>());
            g_object_unref(store.cast());
            add_text_column(view, c"Name", 0, true);

            let column = gtk_tree_view_get_column(view.cast(), 0);
            assert!(!column.is_null(), "the column was not appended");

            let mut sizing: c_int = -1;
            g_object_get(
                column.cast(),
                c"sizing".as_ptr(),
                &mut sizing as *mut c_int,
                ptr::null::<c_char>(),
            );
            assert_eq!(
                sizing, GTK_TREE_VIEW_COLUMN_FIXED,
                "the column grows with its content instead of staying a fixed width"
            );

            let mut fixed_width: c_int = -1;
            g_object_get(
                column.cast(),
                c"fixed-width".as_ptr(),
                &mut fixed_width as *mut c_int,
                ptr::null::<c_char>(),
            );
            assert_eq!(
                fixed_width, NAME_COLUMN_WIDTH,
                "the column has no fixed width to cap its size request at"
            );

            gtk_widget_destroy(view);
        }
    }

    /// GNOME HIG (item 174 (c)): a name column fixed to [`NAME_COLUMN_WIDTH`]
    /// clips a longer name with no visual cue that it was cut off. The
    /// renderer needs `ellipsize` set to mark the cut with "…". Needs a
    /// display; skips without one.
    #[test]
    fn a_name_column_ellipsizes_a_clipped_name() {
        unsafe extern "C" {
            fn gtk_cell_layout_get_cells(
                cell_layout: *mut evo_sys::GtkTreeViewColumn,
            ) -> *mut GList;
        }

        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        // SAFETY: a plain list store and tree view, both destroyed (the
        // store's one reference moves to the view) before returning.
        unsafe {
            let mut types = [G_TYPE_STRING];
            let store = gtk_list_store_newv(1, types.as_mut_ptr());
            let view = gtk_tree_view_new_with_model(store.cast::<GtkTreeModel>());
            g_object_unref(store.cast());
            add_text_column(view, c"Name", 0, true);

            let column = gtk_tree_view_get_column(view.cast(), 0);
            assert!(!column.is_null(), "the column was not appended");

            let cells = gtk_cell_layout_get_cells(column);
            assert!(!cells.is_null(), "the column has no cell renderer");
            let renderer = (*cells).data;
            g_list_free(cells);

            let mut ellipsize: c_int = -1;
            g_object_get(
                renderer.cast(),
                c"ellipsize".as_ptr(),
                &mut ellipsize as *mut c_int,
                ptr::null::<c_char>(),
            );
            assert_eq!(
                ellipsize,
                widget::PANGO_ELLIPSIZE_END,
                "the renderer does not mark a clipped name with an ellipsis"
            );

            gtk_widget_destroy(view);
        }
    }

    /// Accessibility (item 174 (d)): the script list and the rules list have
    /// no preceding label to name them for a screen reader through a
    /// `labelled-by` relation, unlike every other field on this page, so
    /// each needs its own accessible name. Needs a display; skips without
    /// one.
    #[test]
    fn the_script_and_rule_lists_have_accessible_names() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        // SAFETY: two plain tree views, both destroyed before returning.
        unsafe {
            let mut types = [G_TYPE_STRING];
            let store = gtk_list_store_newv(1, types.as_mut_ptr());
            let view = gtk_tree_view_new_with_model(store.cast::<GtkTreeModel>());
            g_object_unref(store.cast());
            widget::set_accessible_name(view.cast(), &translate(SCRIPT_LIST_NAME));
            assert_eq!(
                widget::get_accessible_name(view.cast()),
                translate(SCRIPT_LIST_NAME),
                "the script list keeps no name a screen reader can announce"
            );
            gtk_widget_destroy(view);

            let mut rule_types = [G_TYPE_STRING];
            let rules_store = gtk_list_store_newv(1, rule_types.as_mut_ptr());
            let rules_view = gtk_tree_view_new_with_model(rules_store.cast::<GtkTreeModel>());
            g_object_unref(rules_store.cast());
            widget::set_accessible_name(rules_view.cast(), &translate(RULE_LIST_NAME));
            assert_eq!(
                widget::get_accessible_name(rules_view.cast()),
                translate(RULE_LIST_NAME),
                "the rules list keeps no name a screen reader can announce"
            );
            assert_ne!(
                translate(SCRIPT_LIST_NAME),
                translate(RULE_LIST_NAME),
                "the two lists must not share one name"
            );
            gtk_widget_destroy(rules_view);
        }
    }

    /// Offline (item 176): the page must say so at once instead of showing
    /// the generic transport-error sentence a timed-out connection attempt
    /// would otherwise produce. Needs a display; skips without one.
    #[test]
    fn offline_shows_an_offline_message_not_a_generic_load_failure() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        let gtype = register_static::<FiltersPage>();
        // SAFETY: the type is registered; `instance_init` runs `build_widgets`.
        // No network call happens: `LoadOutcome::Offline` is applied directly.
        unsafe {
            let page = g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null());

            apply_outcome(page, LoadOutcome::Offline, None);

            let status = state(page).unwrap().borrow().status;
            let text = read_string(gtk_label_get_text(status.cast())).unwrap_or_default();
            assert_eq!(
                text,
                translate(OFFLINE),
                "the status label does not say the account is offline"
            );

            gtk_widget_destroy(page.cast());
        }
    }

    /// A private directory for one test's cache files, removed again when
    /// the test ends.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "evolution-jmap-filters-page-cache-test-{name}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Item 180(c)'s cold-start half: a page built already offline, with a
    /// cache hit for its account, shows the cached script read-only instead
    /// of the plain offline message. Needs a display; skips without one.
    #[test]
    fn a_cold_start_cache_hit_shows_the_cached_script_read_only() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        let dir = TempDir::new("cold-start-hit");
        cache::save(
            &dir.0,
            "account-1",
            &[cache::CachedScript {
                name: "evolution-jmap".to_owned(),
                text: "keep;\n".to_owned(),
                active: true,
            }],
        )
        .unwrap();

        let gtype = register_static::<FiltersPage>();
        // SAFETY: the type is registered; `instance_init` runs `build_widgets`.
        // No network call happens: `LoadOutcome::Offline` is applied directly.
        unsafe {
            let page = g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null());

            apply_outcome(page, LoadOutcome::Offline, Some((&dir.0, "account-1")));

            let cell = state(page).unwrap();
            assert_eq!(
                cell.borrow().form.entries().len(),
                1,
                "the cached script must show in the list"
            );
            assert!(
                cell.borrow().offline,
                "a cache hit must lock the page like any other offline state"
            );

            let status = cell.borrow().status;
            let text = read_string(gtk_label_get_text(status.cast())).unwrap_or_default();
            assert_eq!(text, translate(OFFLINE));

            let source_view = cell.borrow().source_view;
            assert!(
                !get_editable(source_view),
                "a cached script must not be editable offline"
            );

            gtk_widget_destroy(page.cast());
        }
    }

    /// Item 180(c)'s cold-start half: a successful online load saves what it
    /// found, so a later cold start while offline can show it. Needs a
    /// display; skips without one.
    #[test]
    fn a_successful_load_saves_the_scripts_to_the_cache() {
        use jmap_client::{Client, Credentials};
        use jmap_mock::MockServer;

        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        let dir = TempDir::new("save-on-load");
        let server = MockServer::builder().start();
        let client = Client::connect(server.origin(), Credentials::none()).unwrap();
        let features =
            crate::session_cache::AccountFeatures::from_session(client.session()).unwrap();
        let link = AccountLink::without_refresh(client, features);

        // Seeds one active script, the same way
        // `deactivating_disables_the_controls_before_the_worker_answers` does.
        let loaded = match io::activate_now(
            &link,
            &SavePlan {
                writes: vec![io::ScriptWrite::Create {
                    key: "new".to_owned(),
                    name: "evolution-jmap".to_owned(),
                    text: "keep;\n".to_owned(),
                }],
                destroys: Vec::new(),
                activation: io::ActivationChange::Activate(io::ActivationTarget::Created(
                    "new".to_owned(),
                )),
                deactivate_first: false,
                if_in_state: None,
                baseline: io::Baseline::default(),
            },
        )
        .unwrap()
        {
            ActivateOutcome::Reloaded(loaded) => loaded,
            ActivateOutcome::WrittenReloadFailed(reason) => {
                panic!("seeding the active script failed: {reason}")
            }
        };

        let gtype = register_static::<FiltersPage>();
        // SAFETY: the type is registered; `instance_init` runs `build_widgets`.
        // No network call happens here: the load above already ran.
        unsafe {
            let page = g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null());

            apply_outcome(
                page,
                LoadOutcome::Loaded((Arc::new(link), loaded)),
                Some((&dir.0, "account-1")),
            );

            gtk_widget_destroy(page.cast());
        }

        let cached =
            cache::load(&dir.0, "account-1").expect("a successful load must write the cache");
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].name, "evolution-jmap");
        assert_eq!(cached[0].text, "keep;\n");
        assert!(cached[0].active);
    }

    /// `widget`'s `sensitive` property, the same way
    /// `delegates::page::get_sensitive` reads it.
    ///
    /// # Safety
    ///
    /// `widget` must be a live `GObject` with a `sensitive` property.
    unsafe fn get_sensitive(widget: *mut GtkWidget) -> bool {
        let mut sensitive: gboolean = GTRUE;
        // SAFETY: as the contract of this function.
        unsafe {
            g_object_get(
                widget.cast(),
                c"sensitive".as_ptr(),
                &mut sensitive as *mut gboolean,
                ptr::null::<c_char>(),
            );
        }
        sensitive == GTRUE
    }

    /// A `GtkTextView`'s own `editable` property, for item 180(c)'s tests.
    ///
    /// # Safety
    ///
    /// `view` must be a live `GtkTextView`.
    unsafe fn get_editable(view: *mut GtkWidget) -> bool {
        let mut editable: gboolean = GTRUE;
        // SAFETY: as the contract of this function.
        unsafe {
            g_object_get(
                view.cast(),
                c"editable".as_ptr(),
                &mut editable as *mut gboolean,
                ptr::null::<c_char>(),
            );
        }
        editable == GTRUE
    }

    /// Item 180(a)'s Deactivate mirrors Activate exactly: clicking it writes
    /// at once instead of waiting for the account editor's OK, and every
    /// script-list control stays insensitive, with a progress status, while
    /// that write is in flight. Needs a display; skips without one.
    #[test]
    fn deactivating_disables_the_controls_before_the_worker_answers() {
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
        let link = AccountLink::without_refresh(client, features);

        // Seeds one already-active script, the same way the io-level test
        // `activate_now_deactivates_and_reloads_in_one_call` does.
        let loaded = match io::activate_now(
            &link,
            &SavePlan {
                writes: vec![io::ScriptWrite::Create {
                    key: "new".to_owned(),
                    name: "evolution-jmap".to_owned(),
                    text: "keep;\n".to_owned(),
                }],
                destroys: Vec::new(),
                activation: io::ActivationChange::Activate(io::ActivationTarget::Created(
                    "new".to_owned(),
                )),
                deactivate_first: false,
                if_in_state: None,
                baseline: io::Baseline::default(),
            },
        )
        .unwrap()
        {
            ActivateOutcome::Reloaded(loaded) => loaded,
            ActivateOutcome::WrittenReloadFailed(reason) => {
                panic!("seeding the active script failed: {reason}")
            }
        };

        let gtype = register_static::<FiltersPage>();
        // SAFETY: the type is registered; `instance_init` runs `build_widgets`.
        // No network call happens here: the load above already ran.
        unsafe {
            let page = g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null());

            apply_outcome(page, LoadOutcome::Loaded((Arc::new(link), loaded)), None);

            let (view, status, add, rename, remove, activate, deactivate, check) = {
                let ui = state(page).unwrap().borrow();
                (
                    ui.view,
                    ui.status,
                    ui.add,
                    ui.rename,
                    ui.remove,
                    ui.activate,
                    ui.deactivate,
                    ui.check,
                )
            };

            // Moving the list's cursor to row 0 also selects it (single
            // selection mode), the same way `selecting_a_delegate_enables_
            // revoke` (delegates/page.rs) selects a row with no real click.
            let path = gtk_tree_path_new_from_indicesv([0].as_mut_ptr(), 1);
            gtk_tree_view_set_cursor(view.cast(), path, ptr::null_mut(), GFALSE);
            gtk_tree_path_free(path);

            deactivate_now(page.cast());

            for control in [add, rename, remove, activate, deactivate, check] {
                assert!(
                    !get_sensitive(control),
                    "a control stayed usable while Deactivate's write was in flight"
                );
            }
            let text = read_string(gtk_label_get_text(status.cast())).unwrap_or_default();
            assert_eq!(
                text,
                translate(DEACTIVATING),
                "the status label does not show progress while Deactivate's write is in flight"
            );

            gtk_widget_destroy(page.cast());
        }
    }

    /// Item 180(a)'s Rename mirrors Activate/Deactivate once the name
    /// dialog answers: it writes at once instead of waiting for the account
    /// editor's OK, with every script-list control insensitive and a
    /// progress status while that write is in flight. Needs a display;
    /// skips without one.
    #[test]
    fn renaming_asks_for_a_name_then_disables_the_controls_before_the_worker_answers() {
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
        let link = AccountLink::without_refresh(client, features);

        // Seeds one inactive script, the same way
        // `deactivating_disables_the_controls_before_the_worker_answers`
        // seeds an active one: a rename needs no active script.
        let loaded = match io::activate_now(
            &link,
            &SavePlan {
                writes: vec![io::ScriptWrite::Create {
                    key: "new".to_owned(),
                    name: "evolution-jmap".to_owned(),
                    text: "keep;\n".to_owned(),
                }],
                destroys: Vec::new(),
                activation: io::ActivationChange::Keep,
                deactivate_first: false,
                if_in_state: None,
                baseline: io::Baseline::default(),
            },
        )
        .unwrap()
        {
            ActivateOutcome::Reloaded(loaded) => loaded,
            ActivateOutcome::WrittenReloadFailed(reason) => {
                panic!("seeding the script failed: {reason}")
            }
        };

        let gtype = register_static::<FiltersPage>();
        // SAFETY: the type is registered; `instance_init` runs `build_widgets`.
        // No network call happens here: the load above already ran.
        unsafe {
            let page = g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null());

            apply_outcome(page, LoadOutcome::Loaded((Arc::new(link), loaded)), None);

            let (view, status, add, rename, remove, activate, deactivate, check) = {
                let ui = state(page).unwrap().borrow();
                (
                    ui.view,
                    ui.status,
                    ui.add,
                    ui.rename,
                    ui.remove,
                    ui.activate,
                    ui.deactivate,
                    ui.check,
                )
            };

            let path = gtk_tree_path_new_from_indicesv([0].as_mut_ptr(), 1);
            gtk_tree_view_set_cursor(view.cast(), path, ptr::null_mut(), GFALSE);
            gtk_tree_path_free(path);

            // Runs inside the dialog's own nested main loop: types a new
            // name, then answers OK, the way
            // `destroying_the_page_during_the_name_dialog_keeps_it_until_
            // the_action_ends` answers Add's dialog.
            g_idle_add(Some(answer_rename), ptr::null_mut());
            rename_now(page.cast());

            for control in [add, rename, remove, activate, deactivate, check] {
                assert!(
                    !get_sensitive(control),
                    "a control stayed usable while Rename's write was in flight"
                );
            }
            let text = read_string(gtk_label_get_text(status.cast())).unwrap_or_default();
            assert_eq!(
                text,
                translate(RENAMING),
                "the status label does not show progress while Rename's write is in flight"
            );

            gtk_widget_destroy(page.cast());
        }
    }

    /// Item 180(a)'s last open row: editing a script's text saves it at
    /// once, once the local Sieve parse accepts it and the same debounce
    /// that drives that parse (item 171b) and completion (item 180(e))
    /// fires, instead of waiting for the account editor's OK. Verifies
    /// through a second, independent connection that the server actually
    /// holds the new text, not only that the page's own copy of it changed.
    /// Needs a display; skips without one.
    #[test]
    fn editing_the_script_text_saves_it_once_the_debounce_fires() {
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
        let link = AccountLink::without_refresh(client, features);

        // Seeds one inactive script under a name other than
        // `MANAGED_SCRIPT_NAME`, so the page stays in text mode: rules mode
        // (item 171a) replaces the text view the debounce save needs.
        let loaded = match io::activate_now(
            &link,
            &SavePlan {
                writes: vec![io::ScriptWrite::Create {
                    key: "new".to_owned(),
                    name: "custom".to_owned(),
                    text: "keep;\n".to_owned(),
                }],
                destroys: Vec::new(),
                activation: io::ActivationChange::Keep,
                deactivate_first: false,
                if_in_state: None,
                baseline: io::Baseline::default(),
            },
        )
        .unwrap()
        {
            ActivateOutcome::Reloaded(loaded) => loaded,
            ActivateOutcome::WrittenReloadFailed(reason) => {
                panic!("seeding the script failed: {reason}")
            }
        };

        let gtype = register_static::<FiltersPage>();
        // SAFETY: the type is registered; `instance_init` runs `build_widgets`.
        // No network call happens here: the load above already ran. The
        // window gives the page a real `GdkWindow`, the same as the
        // completion tests need for their buffer edits.
        unsafe {
            let window = gtk_window_new(0);
            let page = g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null());
            gtk_container_add(window.cast(), page.cast());
            gtk_widget_show_all(window);

            apply_outcome(page, LoadOutcome::Loaded((Arc::new(link), loaded)), None);

            let cell = state(page.cast()).expect("a loaded page has state");
            let buffer = cell.borrow().buffer;

            let view = cell.borrow().view;
            let path = gtk_tree_path_new_from_indicesv([0].as_mut_ptr(), 1);
            gtk_tree_view_set_cursor(view.cast(), path, ptr::null_mut(), GFALSE);
            gtk_tree_path_free(path);

            let mut end = GtkTextIter::default();
            gtk_text_buffer_get_end_iter(buffer, &mut end);
            let added = cstring_lossy("stop;\n");
            gtk_text_buffer_insert(buffer, &mut end, added.as_ptr(), -1);

            pump_until(|| cell.borrow().saving_text, Duration::from_secs(5));
            pump_until(|| !cell.borrow().saving_text, Duration::from_secs(5));

            gtk_widget_destroy(window);
        }

        // A second, independent connection: the save must have reached the
        // server, not only the page's own form.
        let verify_client = Client::connect(server.origin(), Credentials::none()).unwrap();
        let verify_features =
            crate::session_cache::AccountFeatures::from_session(verify_client.session()).unwrap();
        let verify_link = AccountLink::without_refresh(verify_client, verify_features);
        let reloaded = io::load(&verify_link).expect("the server still answers SieveScript/get");
        let saved = reloaded
            .scripts
            .iter()
            .find(|script| script.script.name == "custom")
            .expect("the seeded script is still there");
        assert_eq!(
            saved.text, "keep;\nstop;\n",
            "the debounce must have saved the appended line to the server"
        );
    }

    /// Item 180(a)'s Add mirrors Rename once the name dialog answers: it
    /// writes at once instead of waiting for the account editor's OK, with
    /// every script-list control insensitive and a progress status while
    /// that write is in flight. Needs a display; skips without one.
    #[test]
    fn adding_asks_for_a_name_then_disables_the_controls_before_the_worker_answers() {
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
        let link = AccountLink::without_refresh(client, features);

        // Loads with no script at all: Add needs none, unlike Rename and
        // Deactivate's seeded fixtures.
        let loaded = match io::activate_now(
            &link,
            &SavePlan {
                writes: Vec::new(),
                destroys: Vec::new(),
                activation: io::ActivationChange::Keep,
                deactivate_first: false,
                if_in_state: None,
                baseline: io::Baseline::default(),
            },
        )
        .unwrap()
        {
            ActivateOutcome::Reloaded(loaded) => loaded,
            ActivateOutcome::WrittenReloadFailed(reason) => {
                panic!("seeding the empty account failed: {reason}")
            }
        };

        let gtype = register_static::<FiltersPage>();
        // SAFETY: the type is registered; `instance_init` runs `build_widgets`.
        // No network call happens here: the load above already ran.
        unsafe {
            let page = g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null());

            apply_outcome(page, LoadOutcome::Loaded((Arc::new(link), loaded)), None);

            let (status, add, rename, remove, activate, deactivate, check) = {
                let ui = state(page).unwrap().borrow();
                (
                    ui.status,
                    ui.add,
                    ui.rename,
                    ui.remove,
                    ui.activate,
                    ui.deactivate,
                    ui.check,
                )
            };

            // Runs inside the dialog's own nested main loop: answers OK with
            // the suggested name left as is, the way `answer_ok` answers
            // every other dialog with no change needed.
            g_idle_add(Some(answer_ok), ptr::null_mut());
            add_now(page.cast());

            for control in [add, rename, remove, activate, deactivate, check] {
                assert!(
                    !get_sensitive(control),
                    "a control stayed usable while Add's write was in flight"
                );
            }
            let text = read_string(gtk_label_get_text(status.cast())).unwrap_or_default();
            assert_eq!(
                text,
                translate(ADDING),
                "the status label does not show progress while Add's write is in flight"
            );

            gtk_widget_destroy(page.cast());
        }
    }

    /// Item 180(a)'s Remove mirrors Deactivate exactly: clicking it writes
    /// at once instead of waiting for the account editor's OK, and every
    /// script-list control stays insensitive, with a progress status, while
    /// that write is in flight. Needs a display; skips without one.
    #[test]
    fn removing_disables_the_controls_before_the_worker_answers() {
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
        let link = AccountLink::without_refresh(client, features);

        // Seeds one inactive script, the same way `renaming_asks_for_a_name_
        // then_disables_the_controls_before_the_worker_answers` does: a
        // removal needs no active script.
        let loaded = match io::activate_now(
            &link,
            &SavePlan {
                writes: vec![io::ScriptWrite::Create {
                    key: "new".to_owned(),
                    name: "evolution-jmap".to_owned(),
                    text: "keep;\n".to_owned(),
                }],
                destroys: Vec::new(),
                activation: io::ActivationChange::Keep,
                deactivate_first: false,
                if_in_state: None,
                baseline: io::Baseline::default(),
            },
        )
        .unwrap()
        {
            ActivateOutcome::Reloaded(loaded) => loaded,
            ActivateOutcome::WrittenReloadFailed(reason) => {
                panic!("seeding the script failed: {reason}")
            }
        };

        let gtype = register_static::<FiltersPage>();
        // SAFETY: the type is registered; `instance_init` runs `build_widgets`.
        // No network call happens here: the load above already ran.
        unsafe {
            let page = g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null());

            apply_outcome(page, LoadOutcome::Loaded((Arc::new(link), loaded)), None);

            let (view, status, add, rename, remove, activate, deactivate, check) = {
                let ui = state(page).unwrap().borrow();
                (
                    ui.view,
                    ui.status,
                    ui.add,
                    ui.rename,
                    ui.remove,
                    ui.activate,
                    ui.deactivate,
                    ui.check,
                )
            };

            let path = gtk_tree_path_new_from_indicesv([0].as_mut_ptr(), 1);
            gtk_tree_view_set_cursor(view.cast(), path, ptr::null_mut(), GFALSE);
            gtk_tree_path_free(path);

            remove_now(page.cast());

            for control in [add, rename, remove, activate, deactivate, check] {
                assert!(
                    !get_sensitive(control),
                    "a control stayed usable while Remove's write was in flight"
                );
            }
            let text = read_string(gtk_label_get_text(status.cast())).unwrap_or_default();
            assert_eq!(
                text,
                translate(REMOVING),
                "the status label does not show progress while Remove's write is in flight"
            );

            gtk_widget_destroy(page.cast());
        }
    }

    /// Item 180(c)'s "went offline while the page was open" half: going
    /// offline leaves every control insensitive and the buffer unchanged
    /// and non-editable, with the offline note; coming back online restores
    /// both the buffer's editability and the status label. Needs a
    /// display; skips without one.
    #[test]
    fn going_offline_locks_the_page_without_touching_the_buffer() {
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
        let link = AccountLink::without_refresh(client, features);

        // Seeds one editable, inactive script: the same create-only plan
        // `activate_now_deactivates_and_reloads_in_one_call` seeds, minus
        // its activation.
        let loaded = match io::activate_now(
            &link,
            &SavePlan {
                writes: vec![io::ScriptWrite::Create {
                    key: "new".to_owned(),
                    name: "evolution-jmap".to_owned(),
                    text: "keep;\n".to_owned(),
                }],
                destroys: Vec::new(),
                activation: io::ActivationChange::Keep,
                deactivate_first: false,
                if_in_state: None,
                baseline: io::Baseline::default(),
            },
        )
        .unwrap()
        {
            ActivateOutcome::Reloaded(loaded) => loaded,
            ActivateOutcome::WrittenReloadFailed(reason) => {
                panic!("seeding the script failed: {reason}")
            }
        };

        let gtype = register_static::<FiltersPage>();
        // SAFETY: the type is registered; `instance_init` runs `build_widgets`.
        // No network call happens here: the load above already ran.
        unsafe {
            let page = g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null());

            apply_outcome(page, LoadOutcome::Loaded((Arc::new(link), loaded)), None);

            let (
                view,
                status,
                add,
                rename,
                remove,
                activate,
                deactivate,
                check,
                source_view,
                buffer,
            ) = {
                let ui = state(page).unwrap().borrow();
                (
                    ui.view,
                    ui.status,
                    ui.add,
                    ui.rename,
                    ui.remove,
                    ui.activate,
                    ui.deactivate,
                    ui.check,
                    ui.source_view,
                    ui.buffer,
                )
            };

            let path = gtk_tree_path_new_from_indicesv([0].as_mut_ptr(), 1);
            gtk_tree_view_set_cursor(view.cast(), path, ptr::null_mut(), GFALSE);
            gtk_tree_path_free(path);

            assert!(
                get_sensitive(activate),
                "the script's own button stayed insensitive before going offline"
            );
            let text_before = get_text(buffer.cast());

            let cell = state(page).unwrap();
            apply_offline_change(cell, true);

            for control in [
                add,
                rename,
                remove,
                activate,
                deactivate,
                check,
                source_view,
            ] {
                assert!(
                    !get_sensitive(control),
                    "a control stayed usable after the shell went offline"
                );
            }
            assert!(
                !get_editable(source_view),
                "the script stayed editable after the shell went offline"
            );
            assert_eq!(
                get_text(buffer.cast()),
                text_before,
                "going offline changed the buffer text"
            );
            let offline_text = read_string(gtk_label_get_text(status.cast())).unwrap_or_default();
            assert_eq!(
                offline_text,
                translate(OFFLINE),
                "the status label does not say the shell is offline"
            );

            apply_offline_change(cell, false);

            assert!(
                get_sensitive(activate),
                "the script's own button stayed insensitive after reconnecting"
            );
            assert!(
                get_editable(source_view),
                "the script stayed read-only after reconnecting"
            );
            assert_eq!(
                get_text(buffer.cast()),
                text_before,
                "reconnecting changed the buffer text"
            );
            let online_text = read_string(gtk_label_get_text(status.cast())).unwrap_or_default();
            assert_eq!(
                online_text, "",
                "the offline note stayed after reconnecting"
            );

            gtk_widget_destroy(page.cast());
        }
    }

    /// A page whose initial load never finished (offline from a cold start,
    /// or a load that failed) has no link, so a later shell online/offline
    /// change must not touch it: showing the last loaded scripts read-only
    /// from a cold start is a separate, still open, slice (item 180(c)).
    /// Needs a display; skips without one.
    #[test]
    fn a_page_with_no_link_ignores_a_shell_online_change() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        let gtype = register_static::<FiltersPage>();
        // SAFETY: the type is registered; `instance_init` runs `build_widgets`.
        // No network call happens: `LoadOutcome::Offline` is applied directly.
        unsafe {
            let page = g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null());

            apply_outcome(page, LoadOutcome::Offline, None);

            let cell = state(page).unwrap();
            apply_offline_change(cell, true);
            assert!(
                !cell.borrow().offline,
                "a page with no link must not record a shell offline change"
            );

            let text = read_string(gtk_label_get_text(
                state(page).unwrap().borrow().status.cast(),
            ))
            .unwrap_or_default();
            assert_eq!(
                text,
                translate(OFFLINE),
                "the cold-start offline message changed"
            );

            gtk_widget_destroy(page.cast());
        }
    }
}
