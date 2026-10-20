// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `JmapSnoozeShellExtension`: the snooze submenu in the mail window.
//!
//! An `EExtension` on `E_TYPE_SHELL_VIEW`. Evolution makes one instance for
//! each view of each shell window. The extension works only on the view with
//! the name `"mail"`. The `evolution_eui_manager` cfg selects how the
//! extension finds the time to merge the menu:
//!
//! - Evolution < 3.55 waits for the first `"toggled"` signal of the view (the
//!   pattern of the example module). At that time, the `"mail"` action group
//!   of the window is sure to exist. For this extension, only the mail view
//!   sends this signal.
//! - Evolution 3.55+ has no `"toggled"` signal on `EShellView`. A live test
//!   showed this: a connection to the signal logs a GLib-CRITICAL
//!   ("signal 'toggled' is invalid for instance ... EMailShellView") and the
//!   handler never runs. The view switcher that used `GtkToggleAction` was
//!   removed together with the other `GtkAction` code.
//!   `e-rss-shell-view-extension.c` merges its `EUIManager` menu in
//!   `constructed()` without conditions, thus a merge there is safe. But that
//!   file does not show that the *reader* (the `"mail-view"` property) exists
//!   that early: no upstream code needs the reader at construction.
//!   Thus this extension tries the merge in `constructed()` (it costs little
//!   and usually works). If the reader was not there, the extension tries
//!   again on each `"update-actions"` signal (the signal that both versions
//!   have). The merge is idempotent.
//!
//! The sensitivity always follows `EShellView::update-actions`. The shell
//! sends this signal on each change of the selection and of the folder.

use std::ffi::CStr;

use eds_sys::{EExtension, EExtensionClass, e_extension_get_extensible, e_extension_get_type};
#[cfg(evolution_eui_manager)]
use evo_sys::e_shell_view_get_ui_manager;
use evo_sys::{
    EShellView, e_shell_view_get_name, e_shell_view_get_shell_content, e_shell_view_get_type,
};
#[cfg(not(evolution_eui_manager))]
use evo_sys::{
    e_shell_view_get_shell_window, e_shell_window_get_action_group, e_shell_window_get_ui_manager,
};
use glib_sys::{GType, gpointer};
use gobject_sys::{GObject, GObjectClass, g_signal_connect_data};
use jmap_backend_core::callback::gcallback;
use jmap_backend_core::marshal::read_string;
use jmap_backend_core::subclass::{self, ObjectSubclass};
use jmap_backend_core::trampoline::guard;

use crate::snooze::action;

/// The instance: only the state of `EExtension`.
#[repr(C)]
pub struct JmapSnoozeShellExtension {
    parent: EExtension,
}

/// The class: only the state of `EExtensionClass`.
#[repr(C)]
pub struct JmapSnoozeShellExtensionClass {
    parent_class: EExtensionClass,
}

// SAFETY: both structs are #[repr(C)] and start with the structs of EExtension.
// EExtension derives from GObject.
unsafe impl ObjectSubclass for JmapSnoozeShellExtension {
    const NAME: &'static CStr = c"JmapSnoozeShellExtension";
    type Instance = JmapSnoozeShellExtension;
    type Class = JmapSnoozeShellExtensionClass;

    fn parent_type() -> GType {
        // SAFETY: no arguments, and the type initialises itself.
        unsafe { e_extension_get_type() }
    }

    fn class_init_types() -> Vec<GType> {
        // SAFETY: no arguments, and the type initialises itself.
        // The call is here, before registration, because `class_init` can only look the type up.
        vec![unsafe { e_shell_view_get_type() }]
    }

    unsafe fn class_init(class: *mut Self::Class) {
        // SAFETY: `class` starts with `EExtensionClass`.
        unsafe { (*class).parent_class.extensible_type = subclass::registered_type(c"EShellView") };
        let object_class = class.cast::<GObjectClass>();
        // SAFETY: through its parents, `class` starts with GObjectClass.
        unsafe { (*object_class).constructed = Some(constructed) };
    }
}

/// Calls the parent `constructed`. On the mail view, it connects the signals
/// that this menu layer needs. The signal handlers do the remaining work.
#[cfg(not(evolution_eui_manager))]
unsafe extern "C" fn constructed(object: *mut GObject) {
    guard("JmapSnoozeShellExtension::constructed", (), || unsafe {
        // SAFETY: the parent class of a live instance is initialised and alive.
        let parent =
            subclass::parent_class::<GObjectClass>(JmapSnoozeShellExtension::parent_type());
        if let Some(chained) = parent.and_then(|class| class.constructed) {
            chained(object);
        }

        // SAFETY: GObject gives a live instance. The extensible is the shell
        // view of this extension.
        let view: *mut EShellView = e_extension_get_extensible(object.cast::<EExtension>()).cast();
        if read_string(e_shell_view_get_name(view)).as_deref() != Some("mail") {
            return;
        }

        for (signal, handler) in [
            (
                c"toggled",
                on_toggled as unsafe extern "C" fn(*mut GObject, gpointer),
            ),
            (c"update-actions", on_update_actions),
        ] {
            g_signal_connect_data(
                view.cast(),
                signal.as_ptr(),
                gcallback(handler),
                std::ptr::null_mut(),
                None,
                0,
            );
        }
    });
}

/// # Safety
///
/// GObject gives a live instance that is in construction.
#[cfg(evolution_eui_manager)]
unsafe extern "C" fn constructed(object: *mut GObject) {
    guard("JmapSnoozeShellExtension::constructed", (), || unsafe {
        // SAFETY: the parent class of a live instance is initialised and alive.
        let parent =
            subclass::parent_class::<GObjectClass>(JmapSnoozeShellExtension::parent_type());
        if let Some(chained) = parent.and_then(|class| class.constructed) {
            chained(object);
        }

        // SAFETY: GObject gives a live instance. The extensible is the shell
        // view of this extension.
        let view: *mut EShellView = e_extension_get_extensible(object.cast::<EExtension>()).cast();
        if read_string(e_shell_view_get_name(view)).as_deref() != Some("mail") {
            return;
        }

        try_install(view);

        g_signal_connect_data(
            view.cast(),
            c"update-actions".as_ptr(),
            gcallback(on_update_actions as unsafe extern "C" fn(*mut GObject, gpointer)),
            std::ptr::null_mut(),
            None,
            0,
        );
    });
}

/// Gets the manager and the reader from `view` and merges the submenu.
/// It does nothing if the submenu is installed ([`action::install`] is
/// idempotent for each owner) or if the reader does not exist yet.
/// In the second case, `on_update_actions` tries again.
///
/// # Safety
///
/// `view` must be a live shell view named `"mail"`.
#[cfg(evolution_eui_manager)]
unsafe fn try_install(view: *mut EShellView) {
    // SAFETY: the shell view is live per the contract of this function.
    // The view owns the manager and the content.
    unsafe {
        let reader = action::reader_of_content(e_shell_view_get_shell_content(view));
        action::install(view.cast(), reader, e_shell_view_get_ui_manager(view));
    }
}

/// The view is now the active view of the window. At this time, the `"mail"`
/// action group of the window exists and the merge can occur.
/// The merge occurs one time, because `install` is idempotent.
///
/// # Safety
///
/// The GLib signal system calls this; `view` is the shell view that sent the signal.
#[cfg(not(evolution_eui_manager))]
unsafe extern "C" fn on_toggled(view: *mut GObject, _data: gpointer) {
    guard("JmapSnoozeShellExtension::toggled", (), || unsafe {
        // SAFETY: the view that sent the signal is alive. The view owns the
        // window, the manager, the group and the content.
        let shell_view = view.cast::<EShellView>();
        let window = e_shell_view_get_shell_window(shell_view);
        let reader = action::reader_of_content(e_shell_view_get_shell_content(shell_view));
        action::install(
            view,
            reader,
            e_shell_window_get_ui_manager(window),
            e_shell_window_get_action_group(window, c"mail".as_ptr()),
        );
    });
}

/// # Safety
///
/// The GLib signal system calls this; `view` is the shell view that sent the signal.
#[cfg(not(evolution_eui_manager))]
unsafe extern "C" fn on_update_actions(view: *mut GObject, _data: gpointer) {
    guard("JmapSnoozeShellExtension::update-actions", (), || unsafe {
        // SAFETY: the view that sent the signal is alive.
        action::update_sensitivity(view);
    });
}

/// # Safety
///
/// The GLib signal system calls this; `view` is the shell view that sent the signal.
#[cfg(evolution_eui_manager)]
unsafe extern "C" fn on_update_actions(view: *mut GObject, _data: gpointer) {
    guard("JmapSnoozeShellExtension::update-actions", (), || unsafe {
        // SAFETY: the view that sent the signal is alive. `try_install` is
        // idempotent, thus it is safe if `constructed` installed the submenu.
        // It is necessary if the reader did not exist in `constructed`.
        try_install(view.cast());
        action::update_sensitivity(view);
    });
}
