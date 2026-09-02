// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapVacationPage`: a `GtkScrolledWindow` subclass (the interface's
//! prerequisite) implementing `EMailConfigPage`.
//!
//! The first widget subclass in this workspace, and deliberately the smallest
//! possible one: the parent's sizes come from the running type system
//! ([`TypeSizes::of_parent`]), the subclass adds no inline fields, and all of
//! its state lives in one qdata box ([`UiState`]) — so there is no struct
//! layout here for a GTK release to disagree with.
//!
//! Lifecycle: [`create`] instantiates the registered type, and
//! `instance_init` has already built the widgets (they need no
//! configuration); `create` then kicks the async load — connect, fetch the
//! `VacationResponse` — through [`crate::dispatch::spawn_for`], and
//! [`apply_outcome`] fills the widgets or explains itself in the status
//! label. Saving is the `EMailConfigPage` contract: the account editor's OK
//! button calls the interface's async `submit`, which here snapshots the
//! widgets on the main thread, refuses malformed dates immediately, and runs
//! the blocking `VacationResponse/set` on a `GTask` worker thread. A page
//! whose widgets match what was loaded submits nothing: Evolution submits
//! every page on every save, and an untouched page has no business writing.

use std::cell::RefCell;
use std::ffi::{CStr, c_char, c_int};
use std::ptr;
use std::sync::Arc;

use eds_sys::ESource;
use evo_sys::{
    EMailConfigPage, EMailConfigPageInterface, GTK_ORIENTATION_VERTICAL, GtkTextBuffer, GtkWidget,
    e_mail_config_page_get_type, gtk_box_new, gtk_box_pack_start,
    gtk_check_button_new_with_mnemonic, gtk_container_add, gtk_entry_new, gtk_grid_attach,
    gtk_grid_new, gtk_grid_set_column_spacing, gtk_grid_set_row_spacing, gtk_label_new,
    gtk_label_new_with_mnemonic, gtk_label_set_mnemonic_widget, gtk_label_set_text,
    gtk_label_set_xalign, gtk_scrolled_window_get_type, gtk_text_view_get_buffer,
    gtk_text_view_new, gtk_widget_set_hexpand, gtk_widget_set_sensitive,
    gtk_widget_set_tooltip_text, gtk_widget_set_vexpand, gtk_widget_show_all,
};
use gio_sys::{
    GAsyncReadyCallback, GAsyncResult, GCancellable, GTask, g_io_error_quark, g_task_new,
    g_task_propagate_boolean, g_task_return_boolean, g_task_return_error, g_task_run_in_thread,
    g_task_set_task_data,
};
use glib_sys::{GError, GFALSE, GTRUE, GType, g_error_new_literal, gboolean, gpointer};
use gobject_sys::{
    GObject, GObjectClass, GTypeInstance, g_object_get, g_object_get_data, g_object_ref,
    g_object_set, g_object_set_data_full, g_object_unref, g_signal_connect_data, g_type_from_name,
};
use jmap_backend_core::i18n::{N_, translate, translate_static, translate_with};
use jmap_backend_core::subclass::{InterfaceDecl, InterfaceImpl, ObjectSubclass, TypeSizes};
use jmap_backend_core::trampoline::{guard, log_critical};

use crate::dispatch;
use crate::vacation::form::VacationForm;
use crate::vacation::io::{self, AccountLink};

const NAME: &CStr = c"JmapVacationPage";
const STATE_KEY: &CStr = c"jmap-vacation-page-state";

const CONTACTING: &CStr = N_(c"Contacting the mail server…");
const NOT_OFFERED: &CStr = N_(c"This account’s server does not offer a vacation autoresponder.");
// TRANSLATORS: %1$s is the reason the mail server gave.
const LOAD_FAILED: &CStr = N_(c"The autoresponder could not be read: %1$s");
const DATE_HINT: &CStr = N_(c"YYYY-MM-DD, or empty for no limit");

/// Everything the page keeps, boxed as qdata: widget pointers (main thread
/// only) and the account link the load established (shared with `GTask`
/// submit threads through the `Arc`).
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

/// The page's qdata, while the page is alive.
///
/// # Safety
///
/// `page` must be a live `JmapVacationPage`; the reference must not outlive
/// it (callers use it within one main-loop callback).
unsafe fn state<'a>(page: *mut GObject) -> Option<&'a RefCell<UiState>> {
    // SAFETY: set once in instance_init; the cast is the one build_widgets
    // stored it under.
    unsafe {
        g_object_get_data(page, STATE_KEY.as_ptr())
            .cast::<RefCell<UiState>>()
            .as_ref()
    }
}

unsafe extern "C" fn drop_state(data: gpointer) {
    // SAFETY: reclaiming the box build_widgets leaked, exactly once, when
    // GObject clears the qdata at finalization.
    drop(unsafe { Box::from_raw(data.cast::<RefCell<UiState>>()) });
}

pub struct VacationPage;

// SAFETY: the parent is GtkScrolledWindow, whose sizes type_sizes() takes
// from the running type system; nothing is added inline (state is qdata), and
// GTypeInstance/GObjectClass are prefixes of any GObject-derived structs, so
// the trampolines' casts stay inbounds.
unsafe impl ObjectSubclass for VacationPage {
    const NAME: &'static CStr = NAME;
    type Instance = GTypeInstance;
    type Class = GObjectClass;

    fn parent_type() -> GType {
        // GtkScrolledWindow, because that is `EMailConfigPage`'s prerequisite
        // — the running 3.52 refuses the interface on anything less (a GtkBox
        // and a GtkBin parent were each tried and answered with exactly that
        // CRITICAL, naming the first unmet ancestor).
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { gtk_scrolled_window_get_type() }
    }

    fn type_sizes() -> TypeSizes {
        TypeSizes::of_parent(Self::parent_type())
    }

    fn interfaces() -> Vec<InterfaceDecl> {
        vec![InterfaceDecl::filled_by::<PageInterface>()]
    }

    unsafe fn instance_init(instance: *mut Self::Instance) {
        // SAFETY: GObject passes a live instance whose scrolled-window half
        // the parent initialised before this runs.
        unsafe { build_widgets(instance.cast::<GObject>()) };
    }
}

/// The filling of the page's copy of `EMailConfigPageInterface`.
struct PageInterface;

// SAFETY: `EMailConfigPageInterface` is bindgen's binding of the interface
// struct `e_mail_config_page_get_type` names, generated from the same 3.52
// headers, and it leads with `GTypeInterface`.
unsafe impl InterfaceImpl for PageInterface {
    type Vtable = EMailConfigPageInterface;

    fn gtype() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_mail_config_page_get_type() }
    }

    unsafe fn interface_init(vtable: *mut Self::Vtable) {
        // SAFETY: our own copy of the vtable, pre-filled with the interface's
        // defaults (which is what keeps check_complete and commit_changes at
        // upstream's no-ops); nothing else reaches it yet.
        let vtable = unsafe { &mut *vtable };
        vtable.title = translate_static(N_(c"Vacation Responder"));
        // After every page Evolution builds for the account itself (identity,
        // receiving, sending, security all sort below 500 or at it) — a
        // server-side extra belongs at the tail, same neighbourhood as
        // evolution-ews's Out of Office.
        vtable.sort_order = 550;
        vtable.submit = Some(submit);
        vtable.submit_finish = Some(submit_finish);
    }
}

/// A `GtkLabel` beside its input, on `grid`'s row `row`.
///
/// # Safety
///
/// `grid` must be a live grid and `input` a live widget, both on the main
/// thread.
unsafe fn attach_row(grid: *mut GtkWidget, row: c_int, label: &CStr, input: *mut GtkWidget) {
    let text = translate(label);
    let text = std::ffi::CString::new(text).unwrap_or_default();
    // SAFETY: freshly built label, live grid and input, per the contract.
    unsafe {
        let label = gtk_label_new_with_mnemonic(text.as_ptr());
        gtk_label_set_xalign(label.cast(), 1.0);
        gtk_label_set_mnemonic_widget(label.cast(), input);
        gtk_grid_attach(grid.cast(), label, 0, row, 1, 1);
        gtk_widget_set_hexpand(input, GTRUE);
        gtk_grid_attach(grid.cast(), input, 1, row, 1, 1);
    }
}

/// Builds the page's one child — a vertical box with the widgets — and
/// stores the [`UiState`]. Everything starts insensitive: sensitivity is the
/// load's to grant.
///
/// # Safety
///
/// `page` must be a freshly initialised `JmapVacationPage` instance.
unsafe fn build_widgets(page: *mut GObject) {
    // SAFETY: the instance is a GtkScrolledWindow (parent-initialised), which
    // is what the container cast says — GTK3 wraps the box in a viewport on
    // its own; every widget below is freshly constructed and owned by the
    // container it is packed into.
    unsafe {
        let content = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        gtk_container_add(page.cast(), content);

        let status = gtk_label_new(ptr::null());
        gtk_label_set_xalign(status.cast(), 0.0);
        gtk_box_pack_start(content.cast(), status, GFALSE, GFALSE, 0);
        let contacting = std::ffi::CString::new(translate(CONTACTING)).unwrap_or_default();
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

        let hint = std::ffi::CString::new(translate(DATE_HINT)).unwrap_or_default();
        let from_date = gtk_entry_new();
        gtk_widget_set_tooltip_text(from_date, hint.as_ptr());
        attach_row(grid, 0, N_(c"_First day:"), from_date);
        let to_date = gtk_entry_new();
        gtk_widget_set_tooltip_text(to_date, hint.as_ptr());
        attach_row(grid, 1, N_(c"_Last day:"), to_date);
        let subject = gtk_entry_new();
        attach_row(grid, 2, N_(c"_Subject:"), subject);
        let body = gtk_text_view_new();
        gtk_widget_set_vexpand(body, GTRUE);
        attach_row(grid, 3, N_(c"_Message:"), body);
        let buffer = gtk_text_view_get_buffer(body.cast());

        gtk_widget_show_all(page.cast());

        // The detail grid follows the check button, once the load has made
        // the page live at all.
        g_signal_connect_data(
            enabled.cast(),
            c"toggled".as_ptr(),
            Some(std::mem::transmute::<
                unsafe extern "C" fn(*mut GObject, gpointer),
                unsafe extern "C" fn(),
            >(on_toggled)),
            page.cast(),
            None,
            0,
        );

        let ui = Box::new(RefCell::new(UiState {
            link: None,
            baseline: None,
            enabled,
            from_date,
            to_date,
            subject,
            buffer,
            status,
            grid,
        }));
        g_object_set_data_full(
            page,
            STATE_KEY.as_ptr(),
            Box::into_raw(ui).cast(),
            Some(drop_state),
        );
    }
}

/// The check button's `toggled` handler: the detail grid is editable exactly
/// while automatic replies are on (and the page is live at all).
///
/// # Safety
///
/// Called by GLib's signal machinery; `page` is the connection's data
/// pointer, the page the button lives on, alive because its child emitted.
unsafe extern "C" fn on_toggled(_button: *mut GObject, page: gpointer) {
    guard("JmapVacationPage::toggled", (), || {
        // SAFETY: a live page per this function's contract.
        let Some(ui) = (unsafe { state(page.cast()) }) else {
            return;
        };
        let ui = ui.borrow();
        let live = ui.link.as_ref().is_some_and(|link| link.features.vacation);
        // SAFETY: the grid and check button are the page's own live children.
        unsafe {
            let on = live && get_bool(ui.enabled.cast());
            gtk_widget_set_sensitive(ui.grid, if on { GTRUE } else { GFALSE });
        }
    });
}

/// A ref to `source` that may cross onto the worker thread: `GObject`
/// reference counting is thread-safe, and `ESource` getters take the
/// source's own property lock — the same grounds the EDS backends read
/// sources off their worker threads on.
struct SendSource(*mut ESource);
// SAFETY: see above; the wrapper only hands the pointer to code that treats
// it as a live ESource.
unsafe impl Send for SendSource {}

/// Instantiate the (registered) page for the account `connect_source`
/// configures and start its load. NULL when the type is missing, which means
/// [`crate::module::load`] never ran — reported, since a silently absent page
/// is indistinguishable from the gate refusing.
///
/// # Safety
///
/// `connect_source` must be a valid `ESource`; a reference is taken for the
/// load's duration.
pub unsafe fn create(connect_source: *mut ESource) -> *mut EMailConfigPage {
    // SAFETY: NAME is a 'static NUL-terminated string.
    let gtype = unsafe { g_type_from_name(NAME.as_ptr()) };
    if gtype == 0 {
        log_critical("JmapVacationPage: not registered; was the module loaded?");
        return ptr::null_mut();
    }
    // SAFETY: a registered widget type; a GtkWidget comes back floating, and
    // e_mail_config_notebook_add_page's container sinks it.
    let page = unsafe {
        gobject_sys::g_object_new_with_properties(gtype, 0, ptr::null_mut(), ptr::null())
    };
    if page.is_null() {
        return ptr::null_mut();
    }

    // SAFETY: a valid source per this function's contract; the reference is
    // the worker's, released there.
    let source = SendSource(unsafe { g_object_ref(connect_source.cast()) }.cast());
    let work = move || {
        // Moved whole, so the Send wrapper (not its raw field) is what the
        // closure captures.
        let source = source;
        // SAFETY: the reference `create` took keeps the source alive for the
        // whole of this closure.
        let outcome = unsafe { io::connect_account(source.0) }.and_then(|link| {
            let response = io::load(&link)?;
            Ok((Arc::new(link), VacationForm::from_response(&response)))
        });
        // SAFETY: the worker owns the reference `create` took.
        unsafe { g_object_unref(source.0.cast()) };
        outcome
    };
    let finish = |page: *mut GObject, outcome| {
        // SAFETY: `with_strong` hands back the live page.
        unsafe { apply_outcome(page, outcome) };
    };
    // SAFETY: `page` is the live instance just built, still referenced here.
    unsafe { dispatch::spawn_for(page, work, finish) };

    page.cast()
}

/// Fill the widgets from what the load found, or say why not — on the main
/// loop, with the page alive (dispatch's weak reference saw to both).
///
/// # Safety
///
/// `page` must be the live page the load was started for.
unsafe fn apply_outcome(
    page: *mut GObject,
    outcome: Result<(Arc<AccountLink>, VacationForm), String>,
) {
    let Some(ui) = (unsafe { state(page) }) else {
        return;
    };
    let mut ui = ui.borrow_mut();
    match outcome {
        Ok((link, form)) if link.features.vacation => {
            // SAFETY: the page's own live children.
            unsafe {
                set_bool(ui.enabled.cast(), form.enabled);
                set_text(ui.from_date.cast(), &form.from_date);
                set_text(ui.to_date.cast(), &form.to_date);
                set_text(ui.subject.cast(), &form.subject);
                set_text(ui.buffer.cast(), &form.body);
                gtk_label_set_text(ui.status.cast(), c"".as_ptr());
                gtk_widget_set_sensitive(ui.enabled, GTRUE);
                gtk_widget_set_sensitive(ui.grid, if form.enabled { GTRUE } else { GFALSE });
            }
            ui.baseline = Some(form);
            ui.link = Some(link);
        }
        Ok(_) => {
            let text = std::ffi::CString::new(translate(NOT_OFFERED)).unwrap_or_default();
            // SAFETY: the page's own live status label.
            unsafe { gtk_label_set_text(ui.status.cast(), text.as_ptr()) };
        }
        Err(message) => {
            let text = translate_with(LOAD_FAILED, &[&message]);
            let text = std::ffi::CString::new(text).unwrap_or_default();
            // SAFETY: as above.
            unsafe { gtk_label_set_text(ui.status.cast(), text.as_ptr()) };
        }
    }
}

/// What one submit writes, handed to the `GTask` thread: the link keeps the
/// client alive, the patch is already validated and built.
struct SubmitJob {
    link: Arc<AccountLink>,
    patch: serde_json::Value,
}

unsafe extern "C" fn drop_job(data: gpointer) {
    // SAFETY: reclaiming the box `submit` leaked, exactly once, when the task
    // drops its task data.
    drop(unsafe { Box::from_raw(data.cast::<SubmitJob>()) });
}

/// The blocking half, on the task's worker thread.
unsafe extern "C" fn submit_thread(
    task: *mut GTask,
    _source_object: *mut GObject,
    task_data: gpointer,
    _cancellable: *mut GCancellable,
) {
    guard("JmapVacationPage::submit_thread", (), || {
        // SAFETY: the job `submit` attached; borrowed, the task frees it.
        let job = unsafe { &*task_data.cast::<SubmitJob>() };
        match io::save(&job.link, job.patch.clone()) {
            // SAFETY: `task` is the live task this thread func was invoked for.
            Ok(()) => unsafe { g_task_return_boolean(task, GTRUE) },
            Err(message) => {
                let text = std::ffi::CString::new(message).unwrap_or_default();
                // SAFETY: a fresh GError, ownership passed to the task.
                unsafe {
                    let error = g_error_new_literal(g_io_error_quark(), 0, text.as_ptr());
                    g_task_return_error(task, error);
                }
            }
        }
    });
}

/// `EMailConfigPage.submit`: the account editor's save button.
///
/// # Safety
///
/// Called through the interface vtable with a live page; `cancellable` NULL
/// or valid, `callback`/`user_data` GIO's usual pair.
unsafe extern "C" fn submit(
    page: *mut EMailConfigPage,
    cancellable: *mut GCancellable,
    callback: GAsyncReadyCallback,
    user_data: gpointer,
) {
    guard("JmapVacationPage::submit", (), || {
        // SAFETY: a live page per the vfunc contract; the task is released
        // after hand-off (GTask keeps itself alive until it returns).
        let task = unsafe { g_task_new(page.cast(), cancellable, callback, user_data) };

        let job = unsafe { state(page.cast()) }.and_then(|ui| {
            let ui = ui.borrow();
            let link = ui.link.clone()?;
            if !link.features.vacation {
                return None;
            }
            // SAFETY: the page's own live children, read on the main thread.
            let form = unsafe { snapshot(&ui) };
            if ui.baseline.as_ref() == Some(&form) {
                return None; // untouched: nothing to write
            }
            Some((link, form))
        });

        match job {
            None => {
                // SAFETY: completing the fresh task; it self-releases.
                unsafe {
                    g_task_return_boolean(task, GTRUE);
                    g_object_unref(task.cast());
                }
            }
            Some((link, form)) => match form.patch() {
                Err(reason) => {
                    let text = std::ffi::CString::new(translate(reason)).unwrap_or_default();
                    // SAFETY: as above; the error's ownership passes to the task.
                    unsafe {
                        let error = g_error_new_literal(g_io_error_quark(), 0, text.as_ptr());
                        g_task_return_error(task, error);
                        g_object_unref(task.cast());
                    }
                }
                Ok(patch) => {
                    let job = Box::new(SubmitJob { link, patch });
                    // SAFETY: the task is live; the data's ownership passes to
                    // it (drop_job); run_in_thread takes its own reference, so
                    // the local one is released.
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
/// Called through the interface vtable; `result` is the `GTask` [`submit`]
/// made for this page, `error` NULL or writable.
unsafe extern "C" fn submit_finish(
    _page: *mut EMailConfigPage,
    result: *mut GAsyncResult,
    error: *mut *mut GError,
) -> gboolean {
    guard("JmapVacationPage::submit_finish", GFALSE, || {
        // SAFETY: `result` is the task submit created, per the GIO contract
        // that finish receives what the callback was handed.
        unsafe { g_task_propagate_boolean(result.cast::<GTask>(), error) }
    })
}

/// The widgets as data, read on the main thread.
///
/// # Safety
///
/// The `UiState`'s widgets must be alive (they are, while the page is).
unsafe fn snapshot(ui: &UiState) -> VacationForm {
    // SAFETY: live widgets per the contract.
    unsafe {
        VacationForm {
            enabled: get_bool(ui.enabled.cast()),
            from_date: get_text(ui.from_date.cast()),
            to_date: get_text(ui.to_date.cast()),
            subject: get_text(ui.subject.cast()),
            body: get_text(ui.buffer.cast()),
        }
    }
}

/// One string property read: `text` is the content of a `GtkEntry` and of a
/// `GtkTextBuffer` alike, which is what keeps `GtkTextIter` out of the
/// binding surface.
///
/// # Safety
///
/// `object` must be a live GObject with a readable string `text` property.
unsafe fn get_text(object: *mut GObject) -> String {
    let mut value: *mut c_char = ptr::null_mut();
    // SAFETY: per the contract; g_object_get copies the property value, and
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
    // SAFETY: a NUL-terminated copy this call owns; freed after copying.
    let text = unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: the copy g_object_get handed over.
    unsafe { glib_sys::g_free(value.cast()) };
    text
}

/// # Safety
///
/// `object` must be a live GObject with a writable string `text` property.
unsafe fn set_text(object: *mut GObject, text: &str) {
    let text = std::ffi::CString::new(text).unwrap_or_default();
    // SAFETY: per the contract; the property machinery copies the string.
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
