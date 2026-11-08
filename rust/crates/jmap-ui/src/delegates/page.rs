// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapDelegatesPage`: the Delegates page of the account editor. It is a
//! `GtkScrolledWindow` subclass that implements `EMailConfigPage`, as
//! [`crate::vacation::page`] is, and it follows that page in every
//! structural decision. Read that file first: it explains the parent
//! choice, the qdata state box and the asynchronous load.
//!
//! This first slice (ROADMAP item 164) is read-only: it lists the
//! delegates [`crate::delegation::account_delegates`] already finds in the
//! account's own calendars and address books, one row each, with the
//! calendar and address book level named in its own column. Nothing here
//! writes: `submit` always succeeds without a server call. Granting,
//! revoking and the `Principal/query` picker stay a later claim.
//!
//! The list is a `GtkTreeView` over a `GtkListStore` of three text columns,
//! the same construction as [`crate::filters::page`]: rows go in through
//! `gtk_list_store_insert_with_valuesv`, so this file never allocates a
//! `GtkTreeIter`.

use std::cell::RefCell;
use std::ffi::{CStr, c_int};
use std::ptr;
use std::sync::Arc;

use eds_sys::ESource;
use evo_sys::{
    EMailConfigPage, EMailConfigPageInterface, GTK_ORIENTATION_VERTICAL, GtkCellRenderer,
    GtkListStore, GtkTreeModel, GtkWidget, e_mail_config_page_get_type, gtk_box_new,
    gtk_box_pack_start, gtk_cell_renderer_text_new, gtk_container_add, gtk_label_new,
    gtk_label_set_text, gtk_label_set_xalign, gtk_list_store_clear,
    gtk_list_store_insert_with_valuesv, gtk_list_store_newv, gtk_scrolled_window_get_type,
    gtk_tree_view_append_column, gtk_tree_view_column_add_attribute, gtk_tree_view_column_new,
    gtk_tree_view_column_pack_start, gtk_tree_view_column_set_title, gtk_tree_view_new_with_model,
    gtk_tree_view_set_headers_visible, gtk_widget_set_hexpand, gtk_widget_set_sensitive,
    gtk_widget_set_vexpand, gtk_widget_show_all,
};
use gio_sys::{
    GAsyncReadyCallback, GAsyncResult, GCancellable, GTask, g_task_new, g_task_propagate_boolean,
    g_task_return_boolean,
};
use glib_sys::{GFALSE, GTRUE, GType, gpointer};
use gobject_sys::{
    G_TYPE_STRING, GObject, GObjectClass, GTypeInstance, GValue, g_object_ref, g_object_unref,
    g_type_from_name, g_value_init, g_value_set_string, g_value_unset,
};
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::i18n::{N_, translate, translate_static, translate_with};
use jmap_backend_core::object_data::ObjectData;
use jmap_backend_core::subclass::{InterfaceDecl, InterfaceImpl, ObjectSubclass, TypeSizes};
use jmap_backend_core::trampoline::{guard, log_critical};

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

const DELEGATE_COLUMN_TITLE: &CStr = N_(c"Delegate");
const CALENDAR_COLUMN_TITLE: &CStr = N_(c"Calendars");
const BOOK_COLUMN_TITLE: &CStr = N_(c"Address Books");

/// All page state, in a box as qdata.
struct UiState {
    link: Option<Arc<AccountLink>>,
    status: *mut GtkWidget,
    view: *mut GtkWidget,
    store: *mut GtkListStore,
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

        gtk_widget_show_all(page.cast());

        STATE.set(
            page,
            RefCell::new(UiState {
                link: None,
                status,
                view,
                store,
            }),
        );
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
            let (status, store, view) = {
                let ui = cell.borrow();
                (ui.status, ui.store, ui.view)
            };
            // SAFETY: these are live children of the page. No borrow of
            // `cell` is held during these calls.
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
            }
            let mut ui = cell.borrow_mut();
            ui.link = Some(link);
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

    use evo_sys::gtk_widget_destroy;
    use gobject_sys::g_object_get;
    use jmap_backend_core::marshal::read_string;

    use super::*;

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

            drop(ui);
            gtk_widget_destroy(page);
        }
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
}
