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
//! - All of its state is in one qdata box (`UiState`).
//!
//! Thus this file has no struct layout that a GTK release can contradict.
//!
//! Lifecycle: [`create`] makes an instance of the registered type.
//! `instance_init` has already built the widgets (they need no
//! configuration). Then `create` starts the asynchronous load (connect, get
//! the `VacationResponse`) through [`crate::dispatch::spawn_for`].
//! `apply_outcome` fills the widgets, or shows the problem in the status
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
use std::ffi::{CStr, c_int, c_ulong};
use std::ptr;
use std::sync::Arc;

use eds_sys::ESource;
use evo_sys::{
    EMailConfigPage, EMailConfigPageInterface, GTK_ORIENTATION_VERTICAL, GtkTextBuffer, GtkWidget,
    e_date_edit_get_date, e_date_edit_get_entry, e_date_edit_new,
    e_date_edit_set_allow_no_date_set, e_date_edit_set_date, e_date_edit_set_show_time,
    e_date_edit_set_time, e_mail_config_page_get_type, e_shell_get_default, gtk_box_new,
    gtk_box_pack_start, gtk_check_button_new_with_mnemonic, gtk_container_add, gtk_entry_new,
    gtk_grid_attach, gtk_grid_new, gtk_grid_set_column_spacing, gtk_grid_set_row_spacing,
    gtk_label_new, gtk_label_new_with_mnemonic, gtk_label_set_mnemonic_widget, gtk_label_set_text,
    gtk_label_set_xalign, gtk_scrolled_window_get_type, gtk_text_view_get_buffer,
    gtk_text_view_new, gtk_text_view_set_accepts_tab, gtk_widget_set_hexpand,
    gtk_widget_set_sensitive, gtk_widget_set_vexpand, gtk_widget_show_all,
};
use gio_sys::{
    GAsyncReadyCallback, GAsyncResult, GCancellable, GTask, g_io_error_quark, g_task_new,
    g_task_propagate_boolean, g_task_return_boolean, g_task_return_error, g_task_run_in_thread,
    g_task_set_task_data,
};
use glib_sys::{GError, GFALSE, GTRUE, GType, g_error_new_literal, gboolean, gpointer};
use gobject_sys::{
    GObject, GObjectClass, GTypeInstance, g_object_ref, g_object_unref, g_signal_connect_data,
    g_signal_handler_disconnect, g_type_from_name,
};
use jmap_backend_core::callback::gcallback;
use jmap_backend_core::error::cstring_lossy;
use jmap_backend_core::i18n::{N_, translate, translate_static, translate_with};
use jmap_backend_core::object_data::ObjectData;
use jmap_backend_core::subclass::{InterfaceDecl, InterfaceImpl, ObjectSubclass, TypeSizes};
use jmap_backend_core::trampoline::{guard, log_critical};
use jmap_proto::State;

use crate::dispatch;
use crate::link::{self, AccountLink, LoadOutcome, SendSource};
use crate::vacation::form::VacationForm;
use crate::vacation::io;
use crate::widget::{self, get_bool, get_text, set_bool, set_text};

const NAME: &CStr = c"JmapVacationPage";
const STATE: ObjectData<RefCell<UiState>> = ObjectData::new(c"jmap-vacation-page-state");

/// GTK's own `wrap-mode` value with the same meaning as
/// [`crate::widget::PANGO_WRAP_WORD_CHAR`], for the message text view
/// (`gtkenums.h`: NONE = 0, CHAR = 1, WORD = 2, WORD_CHAR = 3; a different
/// enum from Pango's, with different values).
const GTK_WRAP_WORD_CHAR: c_int = 3;
/// `GtkScrolledWindow`'s `hscrollbar-policy` value that keeps the page from
/// ever scrolling sideways; a wrapped label or message line gives up height
/// instead (`gtkscrolledwindow.h`: ALWAYS = 0, AUTOMATIC = 1, NEVER = 2).
const GTK_POLICY_NEVER: c_int = 2;

const CONTACTING: &CStr = N_(c"Contacting the mail server…");
const NOT_OFFERED: &CStr = N_(c"This account’s server does not offer a vacation autoresponder.");
const OFFLINE: &CStr =
    N_(c"You are offline. The autoresponder cannot be read or changed until you reconnect.");
// TRANSLATORS: %1$s is the reason the mail server gave.
const LOAD_FAILED: &CStr = N_(c"The autoresponder could not be read: %1$s");

/// All page state, in a box as qdata. It holds the widget pointers (main
/// thread only) and the account link from the load. The `Arc` shares the
/// link with the `GTask` submit threads.
struct UiState {
    link: Option<Arc<AccountLink>>,
    baseline: Option<VacationForm>,
    /// The `state` of the load's `VacationResponse/get`, sent as `ifInState`
    /// on save so that it does not overwrite a change of another program.
    state: Option<State>,
    enabled: *mut GtkWidget,
    from_date: *mut GtkWidget,
    to_date: *mut GtkWidget,
    subject: *mut GtkWidget,
    buffer: *mut GtkTextBuffer,
    status: *mut GtkWidget,
    grid: *mut GtkWidget,
    /// Item 180(c)'s "went offline while the page was open" half: true once
    /// [`apply_offline_change`] has made this page read-only. A page whose
    /// initial load never finished (`link` is still `None`) never sets this:
    /// it already shows its own offline message.
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

impl Drop for UiState {
    /// Disconnects from the shell singleton. Without this, the shell (which
    /// outlives this page) would call back into freed memory the next time
    /// it goes online or offline (item 180(c)).
    fn drop(&mut self) {
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
    // SAFETY: forwarded; `input` is also the right mnemonic target here.
    unsafe { attach_row_targeted(grid, row, label, input, input) }
}

/// Like [`attach_row`], but points the label's mnemonic relation at
/// `mnemonic_target` instead of `input`.
///
/// An `EDateEdit` is not itself the accessible object a screen reader lands
/// keyboard focus on; its internal `GtkEntry` is. A mnemonic relation set on
/// the `EDateEdit` container leaves that entry with no accessible name at
/// all, even though the visible label and the Alt-key shortcut both work.
///
/// # Safety
///
/// `grid` must be a live grid; `input` and `mnemonic_target` must be live
/// widgets. Use all three on the main thread.
unsafe fn attach_row_targeted(
    grid: *mut GtkWidget,
    row: c_int,
    label: &CStr,
    input: *mut GtkWidget,
    mnemonic_target: *mut GtkWidget,
) {
    let text = translate(label);
    let text = cstring_lossy(&text);
    // SAFETY: the label is new; the grid, input and mnemonic target are live,
    // per the contract.
    unsafe {
        let label = gtk_label_new_with_mnemonic(text.as_ptr());
        gtk_label_set_xalign(label.cast(), 1.0);
        gtk_label_set_mnemonic_widget(label.cast(), mnemonic_target);
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
        // A long auto-reply message must not push the page into scrolling
        // sideways; it wraps or grows the page's height instead.
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
        attach_row_targeted(
            grid,
            0,
            N_(c"_First day:"),
            from_date,
            e_date_edit_get_entry(from_date.cast()).cast(),
        );
        let to_date = new_date_field();
        attach_row_targeted(
            grid,
            1,
            N_(c"_Last day:"),
            to_date,
            e_date_edit_get_entry(to_date.cast()).cast(),
        );
        let subject = gtk_entry_new();
        attach_row(grid, 2, N_(c"_Subject:"), subject);
        let body = gtk_text_view_new();
        gtk_widget_set_vexpand(body, GTRUE);
        // Keyboard (item 174(b)): a plain Tab must move focus to the next
        // field, not insert a tab character into the message.
        gtk_text_view_set_accepts_tab(body.cast(), GFALSE);
        widget::set_enum_named(body.cast(), c"wrap-mode", GTK_WRAP_WORD_CHAR);
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

        // Item 180(c): watch the shell's online state, so a page already
        // open and loaded notices the user going offline at once instead
        // of only on the next save. No shell instance (a headless test, or
        // none started yet) leaves both ids at 0: nothing to watch, nothing
        // for `Drop` to disconnect later.
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

        let ui = RefCell::new(UiState {
            link: None,
            baseline: None,
            state: None,
            enabled,
            from_date,
            to_date,
            subject,
            buffer,
            status,
            grid,
            offline: false,
            shell: shell.cast(),
            shell_online_handler,
            shell_network_handler,
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

/// Item 180(c)'s "went offline while the page was open" half. Leaves the
/// buffer and the other fields untouched: only the check button's and the
/// grid's sensitivity, and the status note, change. A page whose initial
/// load never finished, or did not offer the feature (`link` is `None`),
/// ignores this: it already shows its own offline or "not offered" message.
///
/// # Safety
///
/// `cell` must be the live state of a page whose widgets are still alive.
unsafe fn apply_offline_change(cell: &RefCell<UiState>, offline: bool) {
    {
        let mut ui = cell.borrow_mut();
        if ui.link.is_none() || ui.offline == offline {
            return;
        }
        ui.offline = offline;
    }
    let (enabled, grid, status) = {
        let ui = cell.borrow();
        (ui.enabled, ui.grid, ui.status)
    };
    // SAFETY: these are live children of the page. No borrow of `cell` is
    // held during these calls.
    unsafe {
        if offline {
            gtk_widget_set_sensitive(enabled, GFALSE);
            gtk_widget_set_sensitive(grid, GFALSE);
            let text = cstring_lossy(&translate(OFFLINE));
            gtk_label_set_text(status.cast(), text.as_ptr());
        } else {
            gtk_widget_set_sensitive(enabled, GTRUE);
            let on = get_bool(enabled.cast());
            gtk_widget_set_sensitive(grid, if on { GTRUE } else { GFALSE });
            gtk_label_set_text(status.cast(), c"".as_ptr());
        }
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
    guard("JmapVacationPage::shell-notify", (), || {
        // SAFETY: the page is live: `Drop` disconnects this handler before
        // the page is freed.
        let Some(cell) = (unsafe { state(page.cast()) }) else {
            return;
        };
        // SAFETY: this handler runs on the main loop, the same thread that
        // writes the shell's `online` and `network-available` properties.
        let offline = unsafe { link::shell_is_offline() };
        // SAFETY: as `apply_offline_change`.
        unsafe { apply_offline_change(cell, offline) };
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
            match unsafe { link::connect_account(source.as_ptr()) }.and_then(|link| {
                let loaded = io::load(&link)?;
                let form = VacationForm::from_response(&loaded.response);
                Ok((Arc::new(link), form, loaded.state))
            }) {
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

/// Fills the widgets with the load result, or shows why the load failed.
/// The weak reference of dispatch makes sure that this runs on the main loop
/// and that the page is alive.
///
/// # Safety
///
/// `page` must be the live page of the load.
unsafe fn apply_outcome(
    page: *mut GObject,
    outcome: LoadOutcome<(Arc<AccountLink>, VacationForm, State)>,
) {
    let Some(cell) = (unsafe { state(page) }) else {
        return;
    };
    match outcome {
        LoadOutcome::Loaded((link, form, load_state)) if link.features.vacation => {
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
            ui.state = Some(load_state);
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

/// The data that one submit writes, for the `GTask` thread. The link keeps
/// the client alive. The patch is already validated.
struct SubmitJob {
    link: Arc<AccountLink>,
    patch: serde_json::Value,
    if_in_state: Option<State>,
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
        match io::save(&job.link, job.patch.clone(), job.if_in_state.clone()) {
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
            Some((link, form, ui.state.clone()))
        });

        match job {
            None => {
                // SAFETY: completes the new task and releases the local reference.
                unsafe {
                    g_task_return_boolean(task, GTRUE);
                    g_object_unref(task.cast());
                }
            }
            Some((link, form, if_in_state)) => match form.patch() {
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
                    let job = Box::new(SubmitJob {
                        link,
                        patch,
                        if_in_state,
                    });
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

#[cfg(test)]
mod tests {
    use std::ffi::{c_char, c_void};

    use evo_sys::{gtk_label_get_mnemonic_widget, gtk_label_get_text, gtk_widget_destroy};
    use gobject_sys::g_object_get;
    use jmap_backend_core::marshal::read_string;
    use jmap_client::{Client, Credentials};
    use jmap_mock::MockServer;
    use jmap_proto::Id;

    use super::*;
    use crate::session_cache::AccountFeatures;

    unsafe extern "C" {
        fn gtk_init_check(argc: *mut c_int, argv: *mut *mut *mut c_char) -> glib_sys::gboolean;
        fn gtk_scrolled_window_new(
            hadjustment: *mut c_void,
            vadjustment: *mut c_void,
        ) -> *mut GtkWidget;
        /// The message row's column (`attach_row`'s `input`), row 3.
        fn gtk_grid_get_child_at(grid: *mut GtkWidget, left: c_int, top: c_int) -> *mut GtkWidget;
    }

    /// Layout (item 174 (a)): the message body must not force the page to
    /// scroll sideways. `GTK_WRAP_NONE` (the GtkTextView default) never
    /// wraps, so an unbroken long line in the auto-reply message would
    /// otherwise widen the page past the window. Needs a display; skips
    /// without one.
    #[test]
    fn the_message_body_wraps_instead_of_scrolling_sideways() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        // SAFETY: a plain `GtkScrolledWindow` stands in for the registered
        // `JmapVacationPage` instance; `build_widgets` only uses it as a
        // `GtkContainer` (`gtk_container_add`) and a qdata target, both of
        // which any `GObject` instance supports.
        unsafe {
            let page = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
            build_widgets(page.cast());

            let grid = state(page.cast()).unwrap().borrow().grid;
            let body = gtk_grid_get_child_at(grid, 1, 3);
            let mut wrap_mode: c_int = -1;
            g_object_get(
                body.cast(),
                c"wrap-mode".as_ptr(),
                &mut wrap_mode as *mut c_int,
                ptr::null::<c_char>(),
            );
            assert_eq!(
                wrap_mode, GTK_WRAP_WORD_CHAR,
                "the message body does not break an overlong word"
            );

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

            gtk_widget_destroy(page);
        }
    }

    /// Keyboard (item 174 (b)): a plain Tab in the message body must move
    /// focus to the next field, not insert a tab character. GTK's own
    /// `GtkTextView` default (`accepts-tab: TRUE`) traps keyboard-only
    /// users, who would need the undiscoverable Ctrl+Tab to escape. Needs
    /// a display; skips without one.
    #[test]
    fn the_message_body_lets_tab_move_focus() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        // SAFETY: see `the_message_body_wraps_instead_of_scrolling_sideways`.
        unsafe {
            let page = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
            build_widgets(page.cast());

            let grid = state(page.cast()).unwrap().borrow().grid;
            let body = gtk_grid_get_child_at(grid, 1, 3);
            let mut accepts_tab: gboolean = GTRUE;
            g_object_get(
                body.cast(),
                c"accepts-tab".as_ptr(),
                &mut accepts_tab as *mut gboolean,
                ptr::null::<c_char>(),
            );
            assert_eq!(
                accepts_tab, GFALSE,
                "Tab in the message body inserts a tab character instead of moving focus"
            );

            gtk_widget_destroy(page);
        }
    }

    /// Accessibility (item 174 (d)): the "First day" and "Last day" labels'
    /// mnemonic relation must target each `EDateEdit` field's internal
    /// `GtkEntry`, the widget AT-SPI actually reports as focusable, not the
    /// `EDateEdit` container. The container is not itself a screen reader's
    /// landing point, so a relation aimed at it leaves the entry unnamed.
    /// Needs a display; skips without one.
    #[test]
    fn the_date_field_labels_target_the_entry_not_the_container() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        // SAFETY: see the test above; the same stand-in instance works here.
        unsafe {
            let page = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
            build_widgets(page.cast());

            let grid = state(page.cast()).unwrap().borrow().grid;
            for (row, name) in [(0, "First day"), (1, "Last day")] {
                let label = gtk_grid_get_child_at(grid, 0, row);
                let field = gtk_grid_get_child_at(grid, 1, row);
                let target = gtk_label_get_mnemonic_widget(label.cast());
                let entry = e_date_edit_get_entry(field.cast()).cast();
                assert_eq!(
                    target, entry,
                    "the {name} label's mnemonic relation targets the EDateEdit \
                     container instead of its entry"
                );
            }

            gtk_widget_destroy(page);
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
        // SAFETY: see the tests above; the same stand-in instance works here.
        // No network call happens: `LoadOutcome::Offline` is applied directly.
        unsafe {
            let page = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
            build_widgets(page.cast());

            apply_outcome(page.cast(), LoadOutcome::Offline);

            let status = state(page.cast()).unwrap().borrow().status;
            let text = read_string(gtk_label_get_text(status.cast())).unwrap_or_default();
            assert_eq!(
                text,
                translate(OFFLINE),
                "the status label does not say the account is offline"
            );

            gtk_widget_destroy(page);
        }
    }

    /// `widget`'s `sensitive` property, the same way
    /// `filters::page::get_sensitive` reads it.
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

    /// Item 180(c)'s "went offline while the page was open" half: going
    /// offline leaves the check button and the grid insensitive, with the
    /// offline note, and does not touch the message buffer; coming back
    /// online restores both. Needs a display; skips without one.
    #[test]
    fn going_offline_locks_the_page_without_touching_the_buffer() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        let server = MockServer::builder().start();
        {
            let state = server.state();
            let mut state = state.lock().unwrap();
            let account = state.account_mut(&server.account_id()).unwrap();
            account
                .vacation_response
                .get_mut(&Id::new("singleton"))
                .unwrap()
                .is_enabled = true;
        }
        let client = Client::connect(server.origin(), Credentials::none()).unwrap();
        let features = AccountFeatures::from_session(client.session()).unwrap();
        let link = AccountLink::without_refresh(client, features);
        let loaded = io::load(&link).unwrap();
        let form = VacationForm::from_response(&loaded.response);

        // SAFETY: a plain `GtkScrolledWindow` stands in for the registered
        // `JmapVacationPage` instance, as the tests above do.
        unsafe {
            let page = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
            build_widgets(page.cast());

            apply_outcome(
                page.cast(),
                LoadOutcome::Loaded((Arc::new(link), form, loaded.state)),
            );

            let (enabled, grid, status, buffer) = {
                let ui = state(page.cast()).unwrap().borrow();
                (ui.enabled, ui.grid, ui.status, ui.buffer)
            };

            assert!(
                get_sensitive(enabled),
                "the check button stayed insensitive before going offline"
            );
            assert!(
                get_sensitive(grid),
                "the grid stayed insensitive before going offline"
            );
            let text_before = get_text(buffer.cast());

            let cell = state(page.cast()).unwrap();
            apply_offline_change(cell, true);

            assert!(
                !get_sensitive(enabled),
                "the check button stayed usable after the shell went offline"
            );
            assert!(
                !get_sensitive(grid),
                "the grid stayed usable after the shell went offline"
            );
            assert_eq!(
                get_text(buffer.cast()),
                text_before,
                "going offline changed the message buffer"
            );
            let offline_text = read_string(gtk_label_get_text(status.cast())).unwrap_or_default();
            assert_eq!(
                offline_text,
                translate(OFFLINE),
                "the status label does not say the shell is offline"
            );

            apply_offline_change(cell, false);

            assert!(
                get_sensitive(enabled),
                "the check button stayed insensitive after reconnecting"
            );
            assert!(
                get_sensitive(grid),
                "the grid stayed insensitive after reconnecting"
            );
            assert_eq!(
                get_text(buffer.cast()),
                text_before,
                "reconnecting changed the message buffer"
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
    /// a load that failed, or a server that does not offer the feature) has
    /// no link, so a later shell online/offline change must not touch it.
    /// Needs a display; skips without one.
    #[test]
    fn a_page_with_no_link_ignores_a_shell_online_change() {
        // SAFETY: no arguments. GTK stays on this thread for the whole test.
        if unsafe { gtk_init_check(ptr::null_mut(), ptr::null_mut()) } == GFALSE {
            eprintln!("skipped: no display for GTK");
            return;
        }
        // SAFETY: see the tests above; the same stand-in instance works here.
        // No network call happens: `LoadOutcome::Offline` is applied directly.
        unsafe {
            let page = gtk_scrolled_window_new(ptr::null_mut(), ptr::null_mut());
            build_widgets(page.cast());

            apply_outcome(page.cast(), LoadOutcome::Offline);

            let cell = state(page.cast()).unwrap();
            apply_offline_change(cell, true);
            assert!(
                !cell.borrow().offline,
                "a page with no link must not record a shell offline change"
            );

            let text = read_string(gtk_label_get_text(
                state(page.cast()).unwrap().borrow().status.cast(),
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
