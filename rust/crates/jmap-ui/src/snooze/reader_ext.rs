// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapSnoozeReaderExtension`: the snooze submenu in every mail reader
//! window.
//!
//! The extensible type is the `EMailReader` *interface*, as in
//! `src/modules/mdn/evolution-mdn.c`. One extension thus covers the mail
//! window and the detached message window. `tests/interface_extension.rs`
//! shows that `EExtension` accepts an interface there.
//!
//! One window can hold more than one reader. Evolution 3.52 has three types
//! that implement `EMailReader` and `EExtensible` and that load their own
//! extensions: `EMailShellContent`, the `EMailPanedView` inside it, and
//! `EMailBrowser`. The first two belong to the same window and give the same
//! action group (`e_shell_window_get_action_group (window, "mail")`), so a
//! merge for each reader would add the submenu twice. Evolution 3.60 dropped
//! the interface from `EMailShellContent`, which leaves two readers, but the
//! code must stay correct on both.
//!
//! Thus the extension keys the merge on the *window* of the reader, not on
//! the reader. `action::install` is idempotent for each owner, so the first
//! reader of a window that reports `update-actions` does the merge and every
//! later reader of that window finds the merge done.
//!
//! The merge happens on `EMailReader::update-actions`, never in
//! `constructed`. At `constructed` time the window has no menu layer for the
//! reader yet. By the first signal it does: `e-mail-shell-view.c` loads
//! `E_MAIL_READER_UI_DEFINITION` into the window and then emits this signal,
//! and a browser emits it when it shows its message.

use std::ffi::CStr;
use std::ptr;

use eds_sys::{EExtension, e_extension_get_extensible};
#[cfg(evolution_eui_manager)]
use evo_sys::e_mail_reader_get_ui_manager;
#[cfg(not(evolution_eui_manager))]
use evo_sys::{
    E_MAIL_READER_ACTION_GROUP_STANDARD, EMailBrowser, EShellWindow, GtkUIManager,
    e_mail_browser_get_type, e_mail_browser_get_ui_manager, e_mail_reader_get_action_group,
    e_shell_window_get_type, e_shell_window_get_ui_manager,
};
use evo_sys::{EMailReader, GtkWindow, e_mail_reader_get_type, e_mail_reader_get_window};
use glib_sys::{GType, gpointer, guint};
use gobject_sys::{GObject, g_signal_connect_data};
use jmap_backend_core::callback::gcallback;
use jmap_backend_core::extension::{Extension, ExtensionObject};
#[cfg(not(evolution_eui_manager))]
use jmap_backend_core::marshal::checked_borrow_ptr;
use jmap_backend_core::trampoline::guard;

use crate::snooze::action;

/// The snooze submenu of a mail reader.
pub struct SnoozeReader;

/// The registered `JmapSnoozeReaderExtension` type.
pub type JmapSnoozeReaderExtension = ExtensionObject<SnoozeReader>;

impl Extension for SnoozeReader {
    const TYPE_NAME: &'static CStr = c"JmapSnoozeReaderExtension";
    const EXTENSIBLE_NAME: &'static CStr = c"EMailReader";

    fn extensible_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_mail_reader_get_type() }
    }

    unsafe fn constructed(extension: *mut EExtension) {
        // SAFETY: the contract of this method.
        unsafe { constructed(extension) }
    }
}

/// Connects the one signal that this extension needs. The handler merges the
/// submenu and sets its sensitivity.
unsafe fn constructed(extension: *mut EExtension) {
    // SAFETY: `extension` is live. The extensible is the reader of this
    // extension (an interface pointer is the instance pointer).
    unsafe {
        let reader: *mut EMailReader = e_extension_get_extensible(extension).cast();
        g_signal_connect_data(
            reader.cast(),
            c"update-actions".as_ptr(),
            gcallback(on_update_actions as unsafe extern "C" fn(*mut GObject, guint, gpointer)),
            ptr::null_mut(),
            None,
            0,
        );
    }
    tracing::trace!("snooze: a mail reader loaded the extension");
}

/// Merges the submenu into the menu layer of `window`, if it is not there.
/// `action::install` returns at once when the owner already has the state, or
/// when a part of the menu layer does not exist yet. In the second case the
/// next signal tries again.
///
/// # Safety
///
/// `reader` must be a live `EMailReader` and `window` its live window.
#[cfg(not(evolution_eui_manager))]
unsafe fn try_install(window: *mut GtkWindow, reader: *mut EMailReader) {
    // SAFETY: both objects are live per the contract of this function.
    // The window owns the manager, and the group belongs to the same window.
    unsafe {
        action::install(
            window.cast(),
            reader,
            ui_manager_of(window),
            e_mail_reader_get_action_group(reader, E_MAIL_READER_ACTION_GROUP_STANDARD),
        );
    }
}

/// # Safety
///
/// As the other `try_install`.
#[cfg(evolution_eui_manager)]
unsafe fn try_install(window: *mut GtkWindow, reader: *mut EMailReader) {
    // SAFETY: both objects are live per the contract of this function.
    // On 3.55 and later the reader itself answers for the manager of its
    // window, so no window type test is necessary.
    unsafe { action::install(window.cast(), reader, e_mail_reader_get_ui_manager(reader)) };
}

/// The `GtkUIManager` of a reader window, or NULL for a window this code does
/// not know. Evolution before 3.55 has no `get_ui_manager` on `EMailReader`,
/// and the two window types answer through different calls.
///
/// # Safety
///
/// `window` must be NULL or a live `GtkWindow`.
#[cfg(not(evolution_eui_manager))]
unsafe fn ui_manager_of(window: *mut GtkWindow) -> *mut GtkUIManager {
    // SAFETY: per the contract. Each cast follows its own type test.
    unsafe {
        if let Some(shell_window) =
            checked_borrow_ptr::<GtkWindow, EShellWindow>(window, e_shell_window_get_type())
        {
            return e_shell_window_get_ui_manager(shell_window);
        }
        if let Some(browser) =
            checked_borrow_ptr::<GtkWindow, EMailBrowser>(window, e_mail_browser_get_type())
        {
            return e_mail_browser_get_ui_manager(browser);
        }
        ptr::null_mut()
    }
}

/// This handler has three arguments. `EMailReader::update-actions` has one
/// `G_TYPE_UINT` parameter and uses `g_cclosure_marshal_VOID__UINT`
/// (`e-mail-reader.c:5374-5382`).
///
/// `state` is the `EMailReaderActionGroup` bitmask of the groups to update.
/// This extension ignores it, because it has one submenu and always reads the
/// selection again. But the handler must *declare* the parameter.
/// If not, the bitmask goes into the user-data slot.
/// `tests/signals.rs` tests this shape.
///
/// # Safety
///
/// The GLib signal system calls this; `reader` is the reader that sent the signal.
unsafe extern "C" fn on_update_actions(reader: *mut GObject, _state: guint, _data: gpointer) {
    guard("JmapSnoozeReaderExtension::update-actions", (), || unsafe {
        // SAFETY: the reader that sent the signal is alive, and the window
        // that holds it outlives it.
        let reader: *mut EMailReader = reader.cast();
        let window = e_mail_reader_get_window(reader);
        if window.is_null() {
            return;
        }
        try_install(window, reader);
        action::update_sensitivity(window.cast());
    });
}
