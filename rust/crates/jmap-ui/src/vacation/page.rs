// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapVacationPage`: a `GtkScrolledWindow` subclass that implements
//! `EMailConfigPage`. The interface requires `GtkScrolledWindow` as parent.
//!
//! This is the first widget subclass in this workspace. It is intentionally
//! as small as possible:
//!
//! - The parent sizes come from the running type system
//!   ([`TypeSizes::of_parent`]).
//! - The subclass adds no inline fields.
//! - All of its state is in one qdata box ([`UiState`]).
//!
//! Thus this file has no struct layout that a GTK release can contradict.
//!
//! Lifecycle: [`create`] makes an instance of the registered type.
//! `instance_init` has already built the widgets (they need no
//! configuration). Then `create` starts the asynchronous load (connect, get
//! the `VacationResponse`) through [`crate::dispatch::spawn_for`].
//! [`apply_outcome`] fills the widgets, or shows the problem in the status
//! label.
//!
//! Save follows the `EMailConfigPage` contract. The OK button of the account
//! editor calls the asynchronous `submit` of the interface. Here, `submit`
//! reads the widgets on the main thread and refuses a date range that is not
//! valid immediately. Then it runs the blocking `VacationResponse/set` on a
//! `GTask` worker thread.
//!
//! If the widgets are equal to the loaded values, the page submits nothing.
//! Evolution submits each page on each save, and a page without changes must
//! not write.

use std::cell::RefCell;
use std::ffi::{CStr, c_char, c_int};
use std::ptr;
use std::sync::Arc;

use eds_sys::ESource;
use evo_sys::{
    EMailConfigPage, EMailConfigPageInterface, GTK_ORIENTATION_VERTICAL, GtkTextBuffer, GtkWidget,
    e_date_edit_get_date, e_date_edit_new, e_date_edit_set_allow_no_date_set, e_date_edit_set_date,
    e_date_edit_set_show_time, e_date_edit_set_time, e_mail_config_page_get_type, gtk_box_new,
    gtk_box_pack_start, gtk_check_button_new_with_mnemonic, gtk_container_add, gtk_entry_new,
    gtk_grid_attach, gtk_grid_new, gtk_grid_set_column_spacing, gtk_grid_set_row_spacing,
    gtk_label_new, gtk_label_new_with_mnemonic, gtk_label_set_mnemonic_widget, gtk_label_set_text,
    gtk_label_set_xalign, gtk_scrolled_window_get_type, gtk_text_view_get_buffer,
    gtk_text_view_new, gtk_widget_set_hexpand, gtk_widget_set_sensitive, gtk_widget_set_vexpand,
    gtk_widget_show_all,
};
use gio_sys::{
    GAsyncReadyCallback, GAsyncResult, GCancellable, GTask, g_io_error_quark, g_task_new,
    g_task_propagate_boolean, g_task_return_boolean, g_task_return_error, g_task_run_in_thread,
    g_task_set_task_data,
};
use glib_sys::{GError, GFALSE, GTRUE, GType, g_error_new_literal, gboolean, gpointer};
use gobject_sys::{
    GObject, GObjectClass, GTypeInstance, g_object_get, g_object_ref, g_object_set, g_object_unref,
    g_signal_connect_data, g_type_from_name,
};
use jmap_backend_core::callback::gcallback;
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::i18n::{N_, translate, translate_static, translate_with};
use jmap_backend_core::object_data::ObjectData;
use jmap_backend_core::subclass::{InterfaceDecl, InterfaceImpl, ObjectSubclass, TypeSizes};
use jmap_backend_core::trampoline::{guard, log_critical};

use crate::dispatch;
use crate::link::{self, AccountLink, SendSource};
use crate::vacation::form::VacationForm;
use crate::vacation::io;

const NAME: &CStr = c"JmapVacationPage";
const STATE: ObjectData<RefCell<UiState>> = ObjectData::new(c"jmap-vacation-page-state");

const CONTACTING: &CStr = N_(c"Contacting the mail server…");
const NOT_OFFERED: &CStr = N_(c"This account’s server does not offer a vacation autoresponder.");
// TRANSLATORS: %1$s is the reason the mail server gave.
const LOAD_FAILED: &CStr = N_(c"The autoresponder could not be read: %1$s");

/// All page state, in a box as qdata. It holds the widget pointers (main
/// thread only) and the account link from the load. The `Arc` shares the
/// link with the `GTask` submit threads.
struct UiState {
    link: Option<Arc<AccountLink>>,
    baseline: Option<VacationForm>,
    enabled: *mut GtkWidget,
    from_date: *mut GtkWidget,
    to_date: *mut GtkWidget,
    subject: *mut GtkWidget,
    buffer: *mut GtkTextBuffer,
    status: *mut GtkWidget,
    grid: *mut GtkWidget,
}

/// The qdata of the page, while the page is alive.
///
/// # Safety
///
/// `page` must be a live `JmapVacationPage`. The reference must not live
/// longer than the page (callers use it in one main-loop callback).
unsafe fn state<'a>(page: *mut GObject) -> Option<&'a RefCell<UiState>> {
    // SAFETY: per the contract. instance_init sets the data one time.
    unsafe { STATE.get(page) }
}

pub struct VacationPage;

// SAFETY: the parent is GtkScrolledWindow. type_sizes() gets its sizes from
// the running type system. The subclass adds no inline fields (the state is
// qdata). GTypeInstance and GObjectClass are prefixes of all GObject-derived
// structs, so the casts in the trampolines stay in bounds.
unsafe impl ObjectSubclass for VacationPage {
    const NAME: &'static CStr = NAME;
    type Instance = GTypeInstance;
    type Class = GObjectClass;

    fn parent_type() -> GType {
        // `EMailConfigPage` requires GtkScrolledWindow. Evolution 3.52 refuses
        // the interface on other parents. Tests with GtkBox and GtkBin gave a
        // CRITICAL that names the first missing ancestor.
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

// SAFETY: `EMailConfigPageInterface` is the bindgen binding of the interface
// struct of `e_mail_config_page_get_type`. Bindgen generated it from the same
// 3.52 headers. It starts with `GTypeInterface`.
unsafe impl InterfaceImpl for PageInterface {
    type Vtable = EMailConfigPageInterface;

    fn gtype() -> GType {
        // SAFETY: no arguments, and the type initializes itself.
        unsafe { e_mail_config_page_get_type() }
    }

    unsafe fn interface_init(vtable: *mut Self::Vtable) {
        // SAFETY: this is the page copy of the vtable. It contains the
        // interface defaults, so check_complete and commit_changes keep the
        // upstream defaults. No other code uses it yet.
        let vtable = unsafe { &mut *vtable };
        vtable.title = translate_static(N_(c"Vacation Responder"));
        // The page sorts after identity (100), receiving (200), sending (400)
        // and defaults (500). Composing also uses 550; security (600) is after.
        vtable.sort_order = 550;
        vtable.submit = Some(submit);
        vtable.submit_finish = Some(submit_finish);
    }
}

/// Puts a `GtkLabel` and its input on row `row` of `grid`.
///
/// # Safety
///
/// `grid` must be a live grid and `input` must be a live widget. Use both on
/// the main thread.
unsafe fn attach_row(grid: *mut GtkWidget, row: c_int, label: &CStr, input: *mut GtkWidget) {
    let text = translate(label);
    let text = cstring_lossy(&text);
    // SAFETY: the label is new; the grid and input are live, per the contract.
    unsafe {
        let label = gtk_label_new_with_mnemonic(text.as_ptr());
        gtk_label_set_xalign(label.cast(), 1.0);
        gtk_label_set_mnemonic_widget(label.cast(), input);
        gtk_grid_attach(grid.cast(), label, 0, row, 1, 1);
        gtk_widget_set_hexpand(input, GTRUE);
        gtk_grid_attach(grid.cast(), input, 1, row, 1, 1);
    }
}

/// Builds the only child of the page (a vertical box with the widgets) and
/// stores the [`UiState`]. The check button and the grid start insensitive.
/// The load makes them sensitive.
///
/// # Safety
///
/// `page` must be a freshly initialised `JmapVacationPage` instance.
unsafe fn build_widgets(page: *mut GObject) {
    // SAFETY: the instance is a GtkScrolledWindow that the parent initialized,
    // as the container cast requires. GTK3 adds a viewport around the box.
    // Each widget below is new, and its container owns it.
    unsafe {
        let content = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        gtk_container_add(page.cast(), content);

        let status = gtk_label_new(ptr::null());
        gtk_label_set_xalign(status.cast(), 0.0);
        gtk_box_pack_start(content.cast(), status, GFALSE, GFALSE, 0);
        let contacting = cstring_lossy(&translate(CONTACTING));
        gtk_label_set_text(status.cast(), contacting.as_ptr());

        let enabled = gtk_check_button_new_with_mnemonic({
            static LABEL: &CStr = N_(c"Send _automatic replies");
            translate_static(LABEL)
        });
        gtk_widget_set_sensitive(enabled, GFALSE);
        gtk_box_pack_start(content.cast(), enabled, GFALSE, GFALSE, 6);

        let grid = gtk_grid_new();
        gtk_grid_set_row_spacing(grid.cast(), 6);
        gtk_grid_set_column_spacing(grid.cast(), 12);
        gtk_widget_set_sensitive(grid, GFALSE);
        gtk_box_pack_start(content.cast(), grid, GTRUE, GTRUE, 0);

        // The Evolution date field, not a GtkEntry. It has a calendar popup
        // and a "no date set" state. That state agrees with the nullable
        // `fromDate`/`toDate` of RFC 8621 §8. The page shows days only, no
        // time.
        let from_date = new_date_field();
        attach_row(grid, 0, N_(c"_First day:"), from_date);
        let to_date = new_date_field();
        attach_row(grid, 1, N_(c"_Last day:"), to_date);
        let subject = gtk_entry_new();
        attach_row(grid, 2, N_(c"_Subject:"), subject);
        let body = gtk_text_view_new();
        gtk_widget_set_vexpand(body, GTRUE);
        attach_row(grid, 3, N_(c"_Message:"), body);
        let buffer = gtk_text_view_get_buffer(body.cast());

        gtk_widget_show_all(page.cast());

        // After a successful load, the check button controls the grid.
        g_signal_connect_data(
            enabled.cast(),
            c"toggled".as_ptr(),
            gcallback(on_toggled as unsafe extern "C" fn(*mut GObject, gpointer)),
            page.cast(),
            None,
            0,
        );

        let ui = RefCell::new(UiState {
            link: None,
            baseline: None,
            enabled,
            from_date,
            to_date,
            subject,
            buffer,
            status,
            grid,
        });
        STATE.set(page, ui);
    }
}

/// The `toggled` handler of the check button. The grid is editable only
/// while automatic replies are on and the page is live.
///
/// # Safety
///
/// The GLib signal system calls this function. `page` is the data pointer
/// of the connection: the page of the button. The page is alive, because its
/// child emitted the signal.
unsafe extern "C" fn on_toggled(_button: *mut GObject, page: gpointer) {
    guard("JmapVacationPage::toggled", (), || {
        // SAFETY: the page is live, per the contract of this function.
        let Some(ui) = (unsafe { state(page.cast()) }) else {
            return;
        };
        let ui = ui.borrow();
        let live = ui.link.as_ref().is_some_and(|link| link.features.vacation);
        // SAFETY: the grid and the check button are live children of the page.
        unsafe {
            let on = live && get_bool(ui.enabled.cast());
            gtk_widget_set_sensitive(ui.grid, if on { GTRUE } else { GFALSE });
        }
    });
}

/// Makes the registered page for the account of `connect_source` and starts
/// its load. Returns NULL if the type or the instance is missing.
///
/// A missing type means that [`crate::module::load`] did not run. The
/// function logs this, because a missing page looks the same as a refused
/// gate.
///
/// # Safety
///
/// `connect_source` must be a valid `ESource`. The function keeps a reference
/// to it during the load.
pub unsafe fn create(connect_source: *mut ESource) -> *mut EMailConfigPage {
    // SAFETY: NAME is a 'static NUL-terminated string.
    let gtype = unsafe { g_type_from_name(NAME.as_ptr()) };
    if gtype == 0 {
        log_critical("JmapVacationPage: not registered; was the module loaded?");
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
            let response = io::load(&link)?;
            Ok((Arc::new(link), VacationForm::from_response(&response)))
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
/// The weak reference of dispatch makes sure that this runs on the main loop
/// and that the page is alive.
///
/// # Safety
///
/// `page` must be the live page of the load.
unsafe fn apply_outcome(
    page: *mut GObject,
    outcome: Result<(Arc<AccountLink>, VacationForm), String>,
) {
    let Some(cell) = (unsafe { state(page) }) else {
        return;
    };
    match outcome {
        Ok((link, form)) if link.features.vacation => {
            // Copy the widget pointers in a short borrow, and release it
            // before a widget call. `set_bool` on `enabled` emits `"toggled"`
            // synchronously when the value changes (an enabled responder does
            // this). Then `on_toggled` borrows this `RefCell` again. A
            // `borrow_mut` held across that call causes a panic ("already
            // mutably borrowed").
            let (enabled, from_date, to_date, subject, buffer, status, grid) = {
                let ui = cell.borrow();
                (
                    ui.enabled,
                    ui.from_date,
                    ui.to_date,
                    ui.subject,
                    ui.buffer,
                    ui.status,
                    ui.grid,
                )
            };
            // SAFETY: these are live children of the page. No borrow of
            // `cell` is held during these calls.
            unsafe {
                set_bool(enabled.cast(), form.enabled);
                set_date(from_date, &form.from_date);
                set_date(to_date, &form.to_date);
                set_text(subject.cast(), &form.subject);
                set_text(buffer.cast(), &form.body);
                gtk_label_set_text(status.cast(), c"".as_ptr());
                gtk_widget_set_sensitive(enabled, GTRUE);
                gtk_widget_set_sensitive(grid, if form.enabled { GTRUE } else { GFALSE });
            }
            let mut ui = cell.borrow_mut();
            ui.baseline = Some(form);
            ui.link = Some(link);
        }
        Ok(_) => {
            let text = cstring_lossy(&translate(NOT_OFFERED));
            let status = cell.borrow().status;
            // SAFETY: the status label is a live child of the page.
            unsafe { gtk_label_set_text(status.cast(), text.as_ptr()) };
        }
        Err(message) => {
            let text = translate_with(LOAD_FAILED, &[&message]);
            let text = cstring_lossy(&text);
            let status = cell.borrow().status;
            // SAFETY: as above.
            unsafe { gtk_label_set_text(status.cast(), text.as_ptr()) };
        }
    }
}

/// The data that one submit writes, for the `GTask` thread. The link keeps
/// the client alive. The patch is already validated.
struct SubmitJob {
    link: Arc<AccountLink>,
    patch: serde_json::Value,
}

unsafe extern "C" fn drop_job(data: gpointer) {
    // SAFETY: this frees the box that `submit` leaked. The task calls this one
    // time, when it drops its task data.
    drop(unsafe { Box::from_raw(data.cast::<SubmitJob>()) });
}

/// The blocking part, on the worker thread of the task.
unsafe extern "C" fn submit_thread(
    task: *mut GTask,
    _source_object: *mut GObject,
    task_data: gpointer,
    _cancellable: *mut GCancellable,
) {
    guard("JmapVacationPage::submit_thread", (), || {
        // SAFETY: `submit` attached the job. This is a borrow; the task frees it.
        let job = unsafe { &*task_data.cast::<SubmitJob>() };
        match io::save(&job.link, job.patch.clone()) {
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
    guard("JmapVacationPage::submit", (), || {
        // SAFETY: the page is live, per the vfunc contract. This function
        // releases the task after hand-off (GTask stays alive until it returns).
        let task = unsafe { g_task_new(page.cast(), cancellable, callback, user_data) };

        let job = unsafe { state(page.cast()) }.and_then(|ui| {
            let ui = ui.borrow();
            let link = ui.link.clone()?;
            if !link.features.vacation {
                return None;
            }
            // SAFETY: live children of the page, read on the main thread.
            let form = unsafe { snapshot(&ui) };
            if ui.baseline.as_ref() == Some(&form) {
                return None; // no changes: nothing to write
            }
            Some((link, form))
        });

        match job {
            None => {
                // SAFETY: completes the new task and releases the local reference.
                unsafe {
                    g_task_return_boolean(task, GTRUE);
                    g_object_unref(task.cast());
                }
            }
            Some((link, form)) => match form.patch() {
                Err(reason) => {
                    let text = cstring_lossy(&translate(reason));
                    // SAFETY: as above. The task gets the ownership of the error.
                    unsafe {
                        let error = g_error_new_literal(g_io_error_quark(), 0, text.as_ptr());
                        g_task_return_error(task, error);
                        g_object_unref(task.cast());
                    }
                }
                Ok(patch) => {
                    let job = Box::new(SubmitJob { link, patch });
                    // SAFETY: the task is live and gets the ownership of the
                    // data (drop_job). run_in_thread keeps its own reference,
                    // so this releases the local reference.
                    unsafe {
                        g_task_set_task_data(task, Box::into_raw(job).cast(), Some(drop_job));
                        g_task_run_in_thread(task, Some(submit_thread));
                        g_object_unref(task.cast());
                    }
                }
            },
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
    guard("JmapVacationPage::submit_finish", GFALSE, || {
        // SAFETY: `result` is the task from submit. The GIO contract gives
        // finish the same result that the callback received.
        unsafe { g_task_propagate_boolean(result.cast::<GTask>(), error) }
    })
}

/// Reads the widget values on the main thread.
///
/// # Safety
///
/// The widgets of the `UiState` must be alive (they are alive while the page
/// is alive).
unsafe fn snapshot(ui: &UiState) -> VacationForm {
    // SAFETY: the widgets are live, per the contract.
    unsafe {
        VacationForm {
            enabled: get_bool(ui.enabled.cast()),
            from_date: get_date(ui.from_date),
            to_date: get_date(ui.to_date),
            subject: get_text(ui.subject.cast()),
            body: get_text(ui.buffer.cast()),
        }
    }
}

/// A date-only [`EDateEdit`] that can have no date. This agrees with the
/// nullable date bounds of RFC 8621 §8.
///
/// # Safety
///
/// Use on the main thread only. The new widget is floating, and a container
/// must sink it.
unsafe fn new_date_field() -> *mut GtkWidget {
    // SAFETY: the widget is new, and both setters receive it.
    unsafe {
        let field = e_date_edit_new();
        e_date_edit_set_show_time(field.cast(), GFALSE);
        e_date_edit_set_allow_no_date_set(field.cast(), GTRUE);
        field
    }
}

/// The value of a date field in the [`VacationForm`] format: `YYYY-MM-DD`,
/// or empty for "no date set".
///
/// # Safety
///
/// `field` must be a live `EDateEdit`, on the main thread.
unsafe fn get_date(field: *mut GtkWidget) -> String {
    let (mut year, mut month, mut day) = (0, 0, 0);
    // SAFETY: the field is live, per the contract. The three out-parameters
    // are writable. FALSE means "no date set", which becomes the empty string.
    let is_set =
        unsafe { e_date_edit_get_date(field.cast(), &mut year, &mut month, &mut day) } != GFALSE;
    if is_set {
        format!("{year:04}-{month:02}-{day:02}")
    } else {
        String::new()
    }
}

/// The inverse of `get_date`. An empty or unparseable string clears the field.
///
/// # Safety
///
/// `field` must be a live `EDateEdit`, on the main thread.
unsafe fn set_date(field: *mut GtkWidget, value: &str) {
    let parsed = {
        let mut parts = value.split('-');
        let year = parts.next().and_then(|p| p.parse::<c_int>().ok());
        let month = parts.next().and_then(|p| p.parse::<c_int>().ok());
        let day = parts.next().and_then(|p| p.parse::<c_int>().ok());
        match (year, month, day, parts.next()) {
            (Some(year), Some(month), Some(day), None) => Some((year, month, day)),
            _ => None,
        }
    };
    // SAFETY: the field is live, per the contract. This class uses
    // `set_time(-1)` for "no date set". There is no `_set_date_to_none`.
    unsafe {
        match parsed {
            Some((year, month, day)) => e_date_edit_set_date(field.cast(), year, month, day),
            None => e_date_edit_set_time(field.cast(), -1),
        }
    }
}

/// Reads the string property `text`. Both `GtkEntry` and `GtkTextBuffer`
/// have this property, so the bindings do not need `GtkTextIter`.
///
/// # Safety
///
/// `object` must be a live GObject with a readable string `text` property.
unsafe fn get_text(object: *mut GObject) -> String {
    let mut value: *mut c_char = ptr::null_mut();
    // SAFETY: per the contract. g_object_get copies the property value, and
    // the NULL sentinel ends the varargs.
    unsafe {
        g_object_get(
            object,
            c"text".as_ptr(),
            &mut value as *mut *mut c_char,
            ptr::null::<c_char>(),
        );
    }
    if value.is_null() {
        return String::new();
    }
    // SAFETY: a NUL-terminated copy that this call owns. It is freed below.
    let text = unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: the copy from g_object_get.
    unsafe { glib_sys::g_free(value.cast()) };
    text
}

/// # Safety
///
/// `object` must be a live GObject with a writable string `text` property.
unsafe fn set_text(object: *mut GObject, text: &str) {
    let text = cstring_lossy(text);
    // SAFETY: per the contract. The property system copies the string.
    unsafe {
        g_object_set(
            object,
            c"text".as_ptr(),
            text.as_ptr(),
            ptr::null::<c_char>(),
        );
    }
}

/// # Safety
///
/// `object` must be a live GObject with a readable boolean `active` property.
unsafe fn get_bool(object: *mut GObject) -> bool {
    let mut value: gboolean = GFALSE;
    // SAFETY: per the contract.
    unsafe {
        g_object_get(
            object,
            c"active".as_ptr(),
            &mut value as *mut gboolean,
            ptr::null::<c_char>(),
        );
    }
    value != GFALSE
}

/// # Safety
///
/// `object` must be a live GObject with a writable boolean `active` property.
unsafe fn set_bool(object: *mut GObject, active: bool) {
    // SAFETY: per the contract.
    unsafe {
        g_object_set(
            object,
            c"active".as_ptr(),
            if active { GTRUE } else { GFALSE },
            ptr::null::<c_char>(),
        );
    }
}
